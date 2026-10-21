"""AddOnTool subagent orchestration.

The facade re-exports these names. Shared runtime, registry, and workspace state
are imported lazily inside functions so the package can load without a cycle.
"""
import contextvars
import os
from typing import Optional

try:
    from deepagents.middleware.filesystem import FilesystemPermission
except ImportError:
    class FilesystemPermission:  # type: ignore[no-redef]
        def __init__(self, operations=None, paths=None, mode="allow"):
            self.operations = operations or []
            self.paths = paths or []
            self.mode = mode
from tools import get_native_tool_names

class SubagentsRegistry(dict):
    def _canonical_key(self, key):
        """Canonical registry key for `key` (case-insensitive, incl. prefix)."""
        if isinstance(key, str):
            # Exact hit first (preserves current behavior + golden key order).
            if dict.__contains__(self, key):
                return key
            lowered = key.lower()
            for k in self.keys():
                if isinstance(k, str) and k.lower() == lowered:
                    return k
            if "-" in key:
                prefix = key.split("-")[0]
                if dict.__contains__(self, prefix):
                    return prefix
                lowered_prefix = prefix.lower()
                for k in self.keys():
                    if isinstance(k, str) and k.lower() == lowered_prefix:
                        return k
        return None

    def __contains__(self, key):
        if dict.__contains__(self, key):
            return True
        return self._canonical_key(key) is not None

    def __getitem__(self, key):
        if dict.__contains__(self, key):
            return dict.__getitem__(self, key)
        canonical = self._canonical_key(key)
        if canonical is not None:
            if isinstance(key, str) and "-" in key:
                val = dict(dict.__getitem__(self, canonical))
                val["name"] = key
                return val
            return dict.__getitem__(self, canonical)
        raise KeyError(key)

    def get(self, key, default=None):
        try:
            return self[key]
        except KeyError:
            return default

_SUBAGENTS_REGISTRY = SubagentsRegistry()


def _canonical_subagent_name(name: str) -> str:
    """Resolve user/LLM-supplied subagent name to its canonical registry key.

    Case-insensitive + whitespace-tolerant (`" explore "` -> `"Explore"`).
    Hyphen-suffixed variants (`"Explore-xyz"`) are NOT spawnable — they
    resolve only for runtime lookups via the registry itself. Returns ""
    when there is no match.
    """
    if not isinstance(name, str):
        return ""
    stripped = name.strip()
    if not stripped:
        return ""
    if not _SUBAGENTS_REGISTRY:
        _load_subagents()
    if dict.__contains__(_SUBAGENTS_REGISTRY, stripped):
        return stripped
    lowered = stripped.lower()
    for k in _SUBAGENTS_REGISTRY.keys():
        if isinstance(k, str) and k.lower() == lowered:
            return k
    return ""

_TASK_DEPTH: contextvars.ContextVar[int] = contextvars.ContextVar("task_depth", default=0)

_MAX_TASK_DEPTH = 3

def _workspace_entries() -> list[dict]:
    """Runtime registry entries for delegatable workspace agents.

    Everything (name, description, prompt module, tools, model pin, runtime
    node) comes from agents/workspace_registry.py — this loader adds no agent
    facts of its own.
    """
    from agents.workspace_registry import delegatable_specs

    out: list[dict] = []
    for spec in delegatable_specs():
        system_prompt = ""
        if spec.prompt_module:
            try:
                import importlib
                mod_name, attr = spec.prompt_module.split(":", 1)
                system_prompt = str(getattr(importlib.import_module(mod_name), attr) or "")
            except Exception as e:
                print(f"[SUBAGENT] {spec.key} prompt load skipped: {e}")
        out.append({
            "name": spec.runtime_name or spec.display,
            "description": spec.description,
            "system_prompt": system_prompt,
            "tools": list(dict.fromkeys(get_native_tool_names(spec.key))),
            "model": spec.model,
            "runtime": spec.runtime,
            "state_builder": spec.state_builder,
        })
    return out


def _specialist_entries() -> list[dict]:
    """Runtime registry entries for config-defined specialists."""
    from agents.workspace_registry import _load_specialists

    out: list[dict] = []
    for spec in _load_specialists().values():
        tools = [str(t) for t in spec.tools]
        if os.environ.get("DEEP_AGENTS_EVAL_RUN") == "1":
            # Headless eval: no human to answer ask_user (it would block to its
            # timeout), so drop the interactive tools from every specialist spec.
            tools = [t for t in tools if t not in ("ask_user", "propose_plan")]
        entry = {
            "name": spec.display,
            "kind": spec.kind or "subagent",
            "internal": spec.internal,
            "description": spec.description,
            "when_to_use": spec.when_to_use,
            "system_prompt": spec.system_prompt,
            "tools": list(dict.fromkeys(tools)),
            "skills": list(dict.fromkeys(spec.skills)),
            "backend": spec.backend,
            "thinking": spec.thinking,
            "compaction": dict(spec.compaction or {}),
            "tool_caps": dict(spec.tool_caps or {}),
            "permissions": list(spec.permissions or []),
        }
        if spec.model:
            entry["model"] = spec.model
        if spec.response_format is not None:
            entry["response_format"] = spec.response_format
        out.append(entry)
    return out


def _load_subagents():
    global _SUBAGENTS_REGISTRY
    try:
        # CF1: deterministic tool order — `set()` iteration is hash-randomized
        # across processes, which would make each subagent SystemMessage bytes
        # differ per restart/subprocess and silently invalidate the DeepSeek
        # prefix cache. dict.fromkeys preserves first-occurrence order.
        for s in _workspace_entries():
            _SUBAGENTS_REGISTRY[s["name"]] = s

        # Explicit aliases from the registry (Vision/FreeAgent/general-purpose
        # -> General). Alias specs are copies so prompt/tool resolution matches.
        from agents.workspace_registry import delegatable_specs

        for spec in delegatable_specs():
            base_name = spec.runtime_name or spec.display
            for alias in spec.aliases:
                base = _SUBAGENTS_REGISTRY.get(base_name)
                if base is not None:
                    alias_spec = dict(base)
                    alias_spec["name"] = alias
                    _SUBAGENTS_REGISTRY[alias] = alias_spec

        # True subagents (task-scoped specialists) from agents/specialists/*.json,
        # normalized by the registry. Missing/corrupt files are a silent no-op so
        # the workspace agents always come up.
        for spec in _specialist_entries():
            _SUBAGENTS_REGISTRY[spec["name"]] = spec
    except Exception as e:
        print(f"Error loading modularized subagents: {e}")


def _subagent_only_names() -> list[str]:
    """Names of true specialist subagents (excludes workspace agents/aliases and internal)."""
    if not _SUBAGENTS_REGISTRY:
        _load_subagents()
    return [
        s["name"] for s in _SUBAGENTS_REGISTRY.values()
        if isinstance(s, dict) and s.get("kind") == "subagent" and not s.get("internal")
    ]


def subagent_catalog(filter_kind: str = "") -> str:
    """Render the '- Name — when to use' catalog for prompt injection (internal excluded)."""
    if not _SUBAGENTS_REGISTRY:
        _load_subagents()
    lines = []
    for s in _SUBAGENTS_REGISTRY.values():
        if isinstance(s, dict) and s.get("internal"):
            continue
        if filter_kind and (not isinstance(s, dict) or s.get("kind") != filter_kind):
            continue
        wtu = s.get("when_to_use") or s.get("description", "")
        lines.append(f"- {s.get('name', '?')} — {wtu}")
    return "\n".join(lines)


def _subagent_workspace_chat() -> tuple[str, str, Optional[str]]:
    """Resolve (workspace, chat_id, parent_session) for specialist spawns."""
    from tools import _HARNESS_ROOT, shared_state
    ws = str(shared_state.get("project_path") or _HARNESS_ROOT)
    chat = str(shared_state.get("chat_id") or "")
    parent = None
    try:
        from server.session.log import find_recent_session
        parent = find_recent_session(chat, ws) if chat else None
    except Exception:
        parent = None
    return ws, chat, parent


def call_subagent(name: str = "", task: str = "", action: str = "run",
                  child_id: str = "", message: str = "", wait_seconds: int = 0) -> str:
    """Specialist subagent lifecycle tool (Codex-style spawn/collect/steer/stop).

    Actions:
      run     — spawn and BLOCK until the final report (default; legacy behavior)
      spawn   — create a durable child, run its first turn in the background,
                return its child_id immediately so work continues in parallel
      status  — status + latest result of a spawned child; wait_seconds≤300 polls
      message — steer a child: queued while running, or runs a follow-up turn
      stop    — cooperatively interrupt a running child
      list    — this session's durable children and their statuses

    Only true specialist subagents from agents/specialists/*.json are spawnable
    here; workspace-agent delegation stays on the orchestrator's delegate tool.
    """
    a = (action or "run").strip().lower()
    allowed = _subagent_only_names()

    if a in ("run", "spawn"):
        if not name or not task:
            return "Error: call_subagent requires name and task."
        canonical = _canonical_subagent_name(name)
        if not canonical or canonical not in allowed:
            return f"Error: Unknown subagent '{name}'. Available: {', '.join(allowed)}"
        name = canonical
        if a == "run":
            return _TASK_TOOL_DELEGATE(name=name, task=task)
        # spawn: durable child + background first turn + context pack + skills.
        try:
            from agents.subagent_manager import spawn_continuable
        except Exception as e:
            return f"Error: subagent manager unavailable: {e}"
        context = ""
        try:
            from agents.subagent_context import build_context_pack
            context = build_context_pack(name)
        except Exception:
            context = ""
        ws, chat, parent = _subagent_workspace_chat()
        try:
            from tools import _HARNESS_ROOT
            from tools.skills import skill_pointer_block
            _skills_block = skill_pointer_block(
                (_SUBAGENTS_REGISTRY.get(name) or {}).get("skills"),
                workspace_path=ws,
                harness_root=_HARNESS_ROOT,
            )
        except Exception:
            _skills_block = ""
        if _skills_block:
            context = (context + "\n\n" + _skills_block).strip()
        try:
            r = spawn_continuable(chat, ws, name, task, parent_session=parent, context=context)
        except Exception as e:
            return f"Error: spawn failed: {str(e)[:300]}"
        cid = r.get("child_id", "")
        return (
            f"Subagent '{name}' spawned in the background.\n"
            f"child_id: {cid}\n"
            f"Status: running\n\n"
            f"Keep working on other parts; collect the result with "
            f"call_subagent(action=\"status\", child_id=\"{cid}\", wait_seconds=120). "
            f"Send it more instructions with action=\"message\"; abort with action=\"stop\"."
        )

    if a in ("status", "message", "stop"):
        if not child_id:
            return f"Error: call_subagent action={a} requires child_id."
        ws, _chat, _parent = _subagent_workspace_chat()
        if a == "status":
            try:
                from agents.subagent_manager import child_status
                st = child_status(child_id, wait_seconds=wait_seconds)
            except Exception as e:
                return f"Error: status failed: {e}"
            status = st.get("status", "unknown")
            if status == "unknown":
                return (f"Error: unknown child '{child_id}'. Use action=\"list\" to see "
                        f"this session's subagents.")
            result = str(st.get("result") or "")
            queued = st.get("queued") or 0
            head = (f"Subagent '{st.get('name') or '?'}'\n"
                    f"child_id: {child_id}\nStatus: {status}"
                    + (f" (queued messages: {queued})" if queued else ""))
            if not result:
                return head + "\nResult: (still running — collect again later)"
            return head + "\nResult:\n" + result
        if a == "message":
            if not message:
                return "Error: call_subagent action=message requires message."
            try:
                from agents.subagent_manager import send_message
                r = send_message(child_id, message, ws)
            except Exception as e:
                return f"Error: message failed: {e}"
            if r.get("queued"):
                return f"[OK] Message queued for {child_id} (child is running; it will process it at its next step)."
            if r.get("status") == "error":
                return str(r.get("result") or f"Error: unknown child '{child_id}'")
            return f"[OK] {child_id} replied:\n{str(r.get('result') or '')}"
        # stop
        try:
            from agents.subagent_manager import interrupt
            ok = interrupt(child_id)
        except Exception as e:
            return f"Error: stop failed: {e}"
        return (f"[OK] Interrupt requested for {child_id}." if ok
                else f"Error: unknown child '{child_id}'")

    if a in ("list", "ls"):
        ws, chat, _parent = _subagent_workspace_chat()
        try:
            from agents.subagent_manager import list_agents
            agents = list_agents(ws, chat_id=chat) if chat else []
        except Exception as e:
            return f"Error: list failed: {e}"
        if not agents:
            return "No specialist subagents in this session."
        lines = [f"{len(agents)} subagent(s):"]
        for ag in agents:
            lines.append(f"  - [{str(ag.get('status', '?')).upper()}] {ag.get('name', '?')} "
                         f"child_id={ag.get('child_id', '')} — {str(ag.get('label') or '')[:80]}")
        return "\n".join(lines)

    return ("Error: call_subagent action must be one of: "
            "run, spawn, status, message, stop, list. Got: '{}'".format(action))


def _specialist_backend(name: str) -> str:
    """Resolve the LLM backend for a specialist subagent.

    Precedence: SUBAGENT_MODEL_BACKEND env > active provider mode's
    specialist entry > agents/specialists/*.json spec "backend" > "deepseek".
    Only "free" dispatches to the free pool; every other value rides the paid
    native pydantic-ai path (which itself resolves deepseek | opencode | command).
    """
    if not _SUBAGENTS_REGISTRY:
        _load_subagents()
    spec_backend = str((_SUBAGENTS_REGISTRY.get(name) or {}).get("backend") or "").strip().lower()
    try:
        from llm.provider_modes import specialist_backend as _mode_backend

        return _mode_backend(name, spec_backend)
    except Exception:
        return spec_backend or "deepseek"


def _specialist_call_backend(
    name: str,
    messages: list,
    native_tools=None,
    schema=None,
    model: Optional[str] = None,
    temp: float = 0.2,
    reasoning_effort: Optional[str] = None,
    where: str = "",
):
    """One LLM turn for a specialist subagent on its configured backend.

    A5: the paid path is now a native pydantic-ai full turn — tools are
    executed internally. Returns ``(text, new_lc, reasoning)`` where
    ``new_lc`` is the LangChain transcript slice the caller extends its
    history with (empty for a tool-less final turn is an AIMessage). The
    ``schema`` (structured-output) path returns the validated pydantic
    instance (tool-less specialists). The free pool keeps its single-turn
    ``(text, tool_calls, reasoning)`` triple (caller owns execution).
    """
    # Thinking profile: an explicit caller override wins; otherwise the agent
    # registry spec decides ("inherit" = built-in per-role mapping downstream).
    if not reasoning_effort:
        try:
            from agents.workspace_registry import get_profile

            _eff = str(get_profile(name).get("thinking") or "inherit").strip().lower()
            if _eff and _eff != "inherit":
                reasoning_effort = _eff
        except Exception:
            pass
    # Legacy alias fallback (ui-design/"design" resolve to the UI spec's tier
    # but are not registry keys themselves).
    if not reasoning_effort and str(name or "").strip().lower() in ("ui-design", "design"):
        reasoning_effort = "high"

    if _specialist_backend(name) == "free":
        # Free pool with live streaming to the parent chat bus; one bounded
        # non-streaming retry on mid-stream failure. Returns the canonical
        # (text, tool_calls, reasoning) triple. Raises loudly on total pool
        # failure — no silent paid fallback by design.
        from llm.free_providers import free_native_turn

        prefer_reasoning = None
        try:
            from agents.workspace_registry import thinking_prefer_reasoning

            prefer_reasoning = thinking_prefer_reasoning(reasoning_effort or "inherit")
        except Exception:
            prefer_reasoning = None

        return free_native_turn(
            name,
            messages,
            tools=native_tools or None,
            capability="coding",
            temperature=temp,
            prefer_reasoning=prefer_reasoning,
        )

    if schema is not None:
        # Structured output (tool-less specialist): single validated result.
        from llm.pai_transport import pai_structured

        return pai_structured(
            list(messages),
            schema,
            temp=temp,
            where=where,
        )

    from llm.native_tools import run_native_agent

    if not model:
        # Provider modes may pin a concrete model for the true-specialist tier
        # (command mode -> poolside/laguna-s-2.1-free). Workspace-tier
        # specialists (UI) return None so the runtime resolves the same
        # canonical flash the Dev agent uses.
        try:
            from llm.provider_modes import specialist_model

            model = specialist_model(name=name)
        except Exception:
            model = None

    # NOTE (rewiring fix): _child_prompt_for() never existed — the call
    # raised NameError on every paid-path turn (swallowed upstream as an
    # instant empty turn). Resolve the spec prompt from this module's own
    # registry instead (mirrors agents/subagent_manager._child_system_prompt;
    # local lookup avoids a circular import).
    _spec_sys = None
    try:
        if not _SUBAGENTS_REGISTRY:
            _load_subagents()
        _spec_sys = (_SUBAGENTS_REGISTRY.get(name) or {}).get("system_prompt")
    except Exception:
        _spec_sys = None
    if isinstance(_spec_sys, list):
        _spec_sys = "\n".join(str(s) for s in _spec_sys)

    # Per-run deps: registry compaction overrides + per-agent tool caps; the
    # model context window is filled in by run_native_agent after resolution.
    _deps = None
    try:
        from llm.deps import WorkspaceDeps

        _deps = WorkspaceDeps.from_shared_state(role=name)
        _profile = {}
        try:
            from agents.workspace_registry import get_profile

            _profile = get_profile(name)
        except Exception:
            _profile = {}
        if _profile.get("compaction"):
            _deps.compaction = dict(_profile["compaction"])
        if _profile.get("tool_caps"):
            _deps.tool_caps = dict(_profile["tool_caps"])
    except Exception:
        _deps = None

    return run_native_agent(
        role=name,
        messages=list(messages),
        tools=native_tools,
        model=model,
        temp=temp,
        reasoning_effort=reasoning_effort,
        where=where,
        deps=_deps,
        system_prompt=_spec_sys or "You are a specialized subagent.",
    )

def _classify_error(exc: str) -> str:
    """Map an exception name to an error category label."""
    if not exc:
        return ""
    e = exc.lower()
    if e in ("keyerror", "indexerror", "attributeerror", "typeerror", "valueerror", "zerodivisionerror", "overflowerror", "nan"):
        return "CODE_BUG (Runtime Logic Error)"
    if e in ("syntaxerror", "indentationerror", "modulenotfounderror", "importerror", "nameerror", "referenceerror"):
        return "CODE_BUG (Syntax/Import Error)"
    if e in ("assertionerror",) or "assert" in e:
        return "TEST_FAILURE (Assertion)"
    if e in ("filenotfounderror", "permissionerror", "connectionerror", "connectionrefusederror", "timeouterror", "econnrefused"):
        return "ENV_ISSUE (Missing resource/connectivity)"
    if e.endswith("error") or e.endswith("exception"):
        return "RUNTIME_ERROR"
    return "UNKNOWN"

def _handoff(content, name: str = "subagent") -> str:
    """Return the subagent's full final report to the caller (no truncation)."""
    return str(content or "").strip()

def _tool_in_scope(tool_name: str, tool_names: list) -> bool:
    """Spec allow-list plus scope-visible MCP tools (`mcp__server__tool`).

    MCP defs are appended per scope by get_native_tools(); the spec allow-list
    governs warehouse tools only, so an MCP name is in scope once the MCP
    manager exposed it for this specialist (config `scopes` list).
    """
    n = str(tool_name or "")
    if n.startswith("mcp__"):
        return True
    return n in tool_names or not tool_names


def _maybe_compact_free(name: str, messages: list, backend: str) -> None:
    """Compact an over-budget specialist transcript on the free-pool path.

    The paid native path gets compaction from the ProcessHistory capability;
    free-pool turns are plain single-shot calls, so the spawner owns the
    checkpoint. Mutates ``messages`` in place; never raises.
    """
    if backend != "free" or len(messages) < 4:
        return
    try:
        from llm.model_history import compact_history_native, estimate_history_tokens
        from memory.context_compaction import get_compaction_policy

        overrides = {}
        if _SUBAGENTS_REGISTRY:
            overrides = (_SUBAGENTS_REGISTRY.get(name) or {}).get("compaction") or {}
        policy = get_compaction_policy(str(name or "developer"))
        try:
            if overrides.get("checkpoint_tokens"):
                policy = type(policy)(
                    role=policy.role,
                    checkpoint_trigger=int(overrides["checkpoint_tokens"]),
                    hard_limit=int(overrides.get("hard_limit") or policy.hard_limit),
                    summary_output_tokens=int(
                        overrides.get("summary_output_tokens") or policy.summary_output_tokens
                    ),
                )
        except Exception:
            pass
        # Free pool: clamp the checkpoint to what the likely top pool models
        # can accept (pure ranking; ~150k trigger for the current top-3 coding
        # set). A lower explicit spec override above still wins via min().
        try:
            from agents.workspace_registry import thinking_prefer_reasoning
            from llm.compaction_native import clamp_policy
            from llm.free_providers import likely_min_window

            _thinking = str((_SUBAGENTS_REGISTRY.get(name) or {}).get("thinking") or "inherit")
            _prefer = thinking_prefer_reasoning(_thinking)
            policy = clamp_policy(
                policy, likely_min_window("coding", prefer_reasoning=_prefer, top_n=3)
            )
        except Exception:
            pass
        if estimate_history_tokens(messages) < policy.checkpoint_trigger:
            return

        from llm.native_tools import _run_coro_blocking
        from memory.compaction_engine import run_compaction_summary
        from tools import shared_state

        project_path = str(shared_state.get("project_path") or "")
        engine = _run_coro_blocking(run_compaction_summary(
            list(messages), project_path, role=str(name or "developer"), backend="free"
        )) or {}
        summary = str(engine.get("summary") or "")
        if not summary:
            print(f"[SUBAGENT] {name}: compaction produced no summary — keeping transcript")
            return
        compacted = compact_history_native(
            list(messages),
            policy=policy,
            full_project_map=str(engine.get("full_map") or "") or None,
            summary_text=summary,
            project_memory_text=str(engine.get("project_memory") or ""),
            user_messages=list(engine.get("user_messages") or []),
        )
        messages[:] = compacted
        print(
            f"[SUBAGENT] {name}: compacted transcript "
            f"(one-pass via {engine.get('model')}, "
            f"window {engine.get('window')})"
        )
    except Exception as e:
        print(f"[SUBAGENT] {name}: free-path compaction skipped: {e}")


def task(name: str, task: str, reasoning_effort: Optional[str] = None) -> str:
    """Delegate a specialized subtask to a subagent with full tool access.

    Spawns a subagent with its own system prompt, tools, and model. The subagent
    runs a full agent loop (LLM â†’ tools â†’ observe â†’ repeat) in ISOLATED context.
    The parent agent never sees the subagent's intermediate work â€” only the result.

    Args:
        name: Subagent type (for example Dev or General).
        task: Detailed instruction describing what the subagent should accomplish.
        reasoning_effort: Optional thinking budget override for the subagent run
            (\"max\" / \"high\" / \"medium\" / \"low\" / \"disabled\"). Defaults to
            the registry spec. Use \"disabled\" for purely
            mechanical subagents, \"high\"/\"max\" for deep root-cause work.
    """
    from tools import (
        _HARNESS_ROOT,
        active_permissions,
        active_tool_caps,
        shared_state,
    )
    depth = _TASK_DEPTH.get()
    if depth >= _MAX_TASK_DEPTH:
        return f"Error: Maximum task depth ({_MAX_TASK_DEPTH}) reached. Cannot spawn subagent '{name}' at depth {depth}."

    if not _SUBAGENTS_REGISTRY:
        _load_subagents()

    canonical = _canonical_subagent_name(name)
    if not canonical or canonical not in _SUBAGENTS_REGISTRY:
        allowed = ", ".join(_SUBAGENTS_REGISTRY.keys())
        return f"Error: Unknown subagent '{name}'. Available subagents: {allowed}"
    name = canonical

    spec = _SUBAGENTS_REGISTRY[name]
    sys_prompt = spec.get("system_prompt", "You are a specialized subagent.")
    tool_names = spec.get("tools", [])
    subagent_model = spec.get("model")
    # Only enforce structured output schema if subagent has NO tools.
    # When tools are available, the agent loop handles output naturally.
    schema = spec.get("response_format") if not tool_names else None

    # Native function calling for the generic loop. The schemas are filtered to
    # the spec's tool allow-list; pseudo-tool text is never generated or parsed.
    native_tools = None
    if schema is None and tool_names:
        try:
            from tools._schemas import get_native_tools
            # Scope-aware: includes the agents/specialists allow-list AND any
            # MCP server tools scoped to this specialist (config-driven).
            native_tools = [
                t for t in get_native_tools(scope=name)
                if _tool_in_scope(t.get("function", {}).get("name", ""), tool_names)
            ]
            if not native_tools:
                native_tools = None
        except Exception:
            native_tools = None

    full_sys_prompt = sys_prompt

    # Permissions
    raw_perms = spec.get("permissions", [])
    perms = []
    for p in raw_perms:
        if isinstance(p, dict):
            perms.append(FilesystemPermission(
                operations=p.get("operations", []),
                paths=p.get("paths", []),
                mode=p.get("mode", "allow")
            ))
        elif isinstance(p, FilesystemPermission):
            perms.append(p)

    depth_token = _TASK_DEPTH.set(depth + 1)
    token = active_permissions.set(perms)
    caps_token = active_tool_caps.set(dict(spec.get("tool_caps") or {}) or None)
    try:
        chat_id = shared_state.get("chat_id", "default_chat")

        # Workspace agents route through their own node (registry `runtime`):
        # Dev (dynamic spec/requirements context), General and House (plain
        # state). Specialists (kind="subagent") fall through to the generic
        # native loop below.
        if spec.get("kind") != "subagent" and spec.get("runtime"):
            import importlib
            from sync.state_sync import safe_update_state

            _mod_name, _fn_name = str(spec["runtime"]).split(":", 1)
            _node = getattr(importlib.import_module(_mod_name), _fn_name)
            _project_path = shared_state.get("project_path", _HARNESS_ROOT)

            if spec.get("state_builder") == "dev":
                dev_state = {
                    "client_request": task,
                    "tech_spec": shared_state.get("outputs", {}).get("tech_spec", ""),
                    "requirements": shared_state.get("outputs", {}).get("requirements", ""),
                    "project_path": _project_path,
                    "chat_id": chat_id,
                    "error_count": 0,
                    "test_report": "",
                }
                res_dict = _node(dev_state)

                # Sync outputs back to shared_state
                if "code" in res_dict and res_dict["code"]:
                    safe_update_state({
                        "outputs": {
                            **shared_state.get("outputs", {}),
                            "code": res_dict["code"],
                            "test_report": res_dict.get("test_report", ""),
                        }
                    })

                return _handoff(
                    res_dict.get("agent_report") or res_dict.get("code")
                    or f"{name} completed execution.",
                    name,
                )

            plain_state = {
                "client_request": task,
                "project_path": _project_path,
                "chat_id": chat_id,
            }
            res_dict = _node(plain_state)
            return _handoff(
                res_dict.get("agent_report") or f"{name} completed with no output.",
                name,
            )

        # All other subagents use the same native loop with a fresh transcript.
        resolved_task = task

        # P0-LLM: native List[ModelMessage] history (no LangChain messages).
        from pydantic_ai.messages import (
            ModelRequest,
            ModelResponse,
            SystemPromptPart,
            TextPart,
            ThinkingPart,
            ToolCallPart,
            UserPromptPart,
        )
        messages: list = []
        if full_sys_prompt:
            messages.append(ModelRequest(parts=[SystemPromptPart(content=full_sys_prompt)]))
        # Context pack (specialists only): project identity + map + parent
        # goal/plan + latest failure, injected before the task so the child
        # starts oriented without re-discovery. Workspace agents never get it.
        if spec.get("kind") == "subagent":
            try:
                from agents.subagent_context import build_context_pack
                _ctx_pack = build_context_pack(name)
            except Exception:
                _ctx_pack = ""
            if _ctx_pack:
                messages.append(ModelRequest(parts=[UserPromptPart(content=_ctx_pack)]))
            # Skills pointer (progressive disclosure): the spec names the skills.
            # Only name + description + absolute path go in-context; the agent
            # reads the full SKILL.md when the task matches.
            try:
                from tools.skills import skill_pointer_block
                _skills_block = skill_pointer_block(
                    spec.get("skills"),
                    workspace_path=str(shared_state.get("project_path") or ""),
                    harness_root=_HARNESS_ROOT,
                )
            except Exception:
                _skills_block = ""
            if _skills_block:
                messages.append(ModelRequest(parts=[UserPromptPart(content=_skills_block)]))
        messages.append(ModelRequest(parts=[UserPromptPart(content=resolved_task)]))

        # Vision: inject user-attached images stashed by the API layer.
        try:
            from llm.deps import WorkspaceDeps
            _sub_deps = WorkspaceDeps.from_shared_state(role=name, chat_id=chat_id)
            from tools.vision_channel import drain_attachment_images_native
            drain_attachment_images_native(messages, ctx=_sub_deps)
        except Exception:
            pass

        # ── Child session log (P1): fresh child per delegation, lineage to the
        # orchestrator's session (best-effort; never breaks the loop) ──
        sub_sess = None
        try:
            from uuid import uuid4
            from server.session.log import get_session, find_recent_session
            ws = shared_state.get("project_path") or _HARNESS_ROOT
            parent_path = find_recent_session(chat_id, ws) if chat_id else None
            sub_sess = get_session(
                f"sub-{chat_id}-{name}-{uuid4().hex[:6]}", ws,
                parent_session=parent_path,
            )
            sub_sess.append_message("user", resolved_task, source="user")
            # Announce the spawn for the UI (trace step → clickable block).
            # Observability only — the loop below is untouched.
            try:
                from agents.subagent_manager import _task_label, _log_spawn
                _log_spawn(sub_sess.get_header().chat_id, name, _task_label(resolved_task))
            except Exception:
                pass
        except Exception:
            sub_sess = None

        # Subagent loop below calls _specialist_call_backend per turn (native);
        # the legacy invoke_messages_with_fallback path is retired (P0-LLM).

        # Static per-role thinking: subagents get no explicit override; the
        # role mapping in _build_model_settings decides (dev/coder=high, rest=low).
        subagent_effort = reasoning_effort

        # ── Subagent loop: LLM → tools → observe → repeat ──
        # There is no turn ceiling. Cancellation, provider failure, a final
        # response, or an explicit structured-output request are the lifecycle
        # boundaries; context checkpoints handle long transcripts.
        # ── Subagent loop: single shared engine (agents/specialist_loop.py) ──
        # There is no turn ceiling. Cancellation, provider failure, a final
        # response, or an explicit structured-output request are the lifecycle
        # boundaries; context checkpoints handle long transcripts.
        try:
            _backend = _specialist_backend(name)
            # Per-turn deps (role + caller chat) so scope-aware tools
            # (update_plan, vision) resolve the specialist, not the caller.
            from agents.specialist_loop import (
                SpecialistLoopHooks,
                run_specialist_loop,
                specialist_turn_deps,
            )
            _loop_deps = specialist_turn_deps(name)

            def _task_backend(msgs, iteration, last_tool):
                return _specialist_call_backend(
                    name,
                    msgs,
                    native_tools=native_tools,
                    schema=schema,
                    model=subagent_model,
                    reasoning_effort=subagent_effort,
                    where=f"{name} iter {iteration}: {last_tool or 'start'}",
                )

            def _task_execute(tc):
                s = _subagent_execute_one(tc, tool_names, _loop_deps)
                return (s, s, False)

            def _task_append_final(transcript, text, reasoning):
                _parts: list = []
                if reasoning:
                    _parts.append(ThinkingPart(content=reasoning))
                _parts.append(TextPart(content=text))
                transcript.append(ModelResponse(parts=_parts))

            def _task_append_calls(transcript, text, reasoning, calls):
                _parts: list = []
                if reasoning:
                    _parts.append(ThinkingPart(content=reasoning))
                if text:
                    _parts.append(TextPart(content=text))
                for tc in calls:
                    _parts.append(ToolCallPart(
                        tool_name=tc["tool"],
                        args=tc.get("args") or {},
                        tool_call_id=tc.get("id", ""),
                    ))
                transcript.append(ModelResponse(parts=_parts))

            _hooks = SpecialistLoopHooks(
                id_prefix="sub",
                parallel=True,
                schema=schema,
                raise_backend_errors=True,
                note_vision=False,
                scope_filter=lambda tc: (
                    None
                    if _tool_in_scope(tc.get("tool", ""), tool_names)
                    else f"[SKIPPED] Tool '{tc.get('tool', '')}' not in subagent permissions."
                ),
                classify_error=lambda t, r: bool(r) or t.lstrip().startswith(
                    ("ERROR:", "[SKIPPED]", "Error:")),
                log_assistant=lambda b, s: sub_sess.append_assistant(
                    b, stop_reason=s),
                log_tool_result=lambda c, i, t, e: sub_sess.append_tool_result(
                    c, tool_call_id=i, tool_name=t, is_error=e),
                log_system=None,
                append_final=_task_append_final,
                append_with_calls=_task_append_calls,
                compact=lambda t: _maybe_compact_free(name, t, _backend),
                deps=_loop_deps,
            )
            _outcome = run_specialist_loop(
                name=name,
                transcript=messages,
                backend_fn=_task_backend,
                execute_fn=_task_execute,
                hooks=_hooks,
            )
            content = _outcome.content

            # No budget checks here; subagents are managed by LoopGuard and structural loops.

            # [Security & performance are enforced by the tiered CI gate on push
            #  (fast-checks / staging / release-heavy) — the gate replaces the former DevSecOps
            #  host-validation step.]

        except Exception:
            raise

    finally:
        active_permissions.reset(token)
        active_tool_caps.reset(caps_token)
        _TASK_DEPTH.reset(depth_token)

    # Specialist subagents (kind="subagent"): the final assistant message IS
    # the tool result — verbatim, opencode/Claude-Code style. The parent-side
    # execution boundary applies the harness-standard cap + VFS offload.
    if spec.get("kind") == "subagent":
        return content

    # Full output beyond the handoff cap is kept on disk (VFS) with a pointer in the
    # report; the handoff itself stays compact so the orchestrator's context never
    # eats a multi-KB dump.
    return _handoff(content, name)

# _is_native_slice lives in agents/specialist_loop.py (single shared copy).


def _subagent_execute_one(tool_call: dict, tool_names: list, deps=None) -> str:
    """Thread-pool worker for a subagent's parallel batch. Only PARALLEL_SAFE
    tools reach this path (enforced by run_tool_calls). ``deps`` carries the
    specialist identity so scope-aware tools resolve it (None = parent)."""
    tool_name = str(tool_call.get("tool", "") or "")
    args = tool_call.get("args") or {}
    if not isinstance(args, dict):
        args = {}
    if not _tool_in_scope(tool_name, tool_names):
        return f"[SKIPPED] Tool '{tool_name}' not in subagent permissions."
    try:
        from tools import execute_tool_detailed
        execution = execute_tool_detailed(tool_name, args, deps=deps)
        return str(execution.content)
    except Exception as e:
        return f"ERROR: {str(e)[:200]}"


# Alias captured after task() is defined; call_subagent's `task` parameter
# would otherwise shadow the module-level function at call time.
_TASK_TOOL_DELEGATE = task

