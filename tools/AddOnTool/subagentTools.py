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
    def __contains__(self, key):
        if super().__contains__(key):
            return True
        if isinstance(key, str) and "-" in key:
            prefix = key.split("-")[0]
            if prefix in self:
                return True
        return False

    def __getitem__(self, key):
        if super().__contains__(key):
            return super().__getitem__(key)
        if isinstance(key, str) and "-" in key:
            prefix = key.split("-")[0]
            if prefix in self:
                val = dict(super().__getitem__(prefix))
                val["name"] = key
                return val
        raise KeyError(key)

    def get(self, key, default=None):
        try:
            return self[key]
        except KeyError:
            return default

_SUBAGENTS_REGISTRY = SubagentsRegistry()

_TASK_DEPTH: contextvars.ContextVar[int] = contextvars.ContextVar("task_depth", default=0)

_MAX_TASK_DEPTH = 3

def _load_subagents():
    global _SUBAGENTS_REGISTRY
    try:
        from agents.developer_prompt import _STATIC_SYSTEM_TEMPLATE as _DEV_SYSTEM_TEMPLATE
        from agents.general_prompt import _GENERAL_SYSTEM_TEMPLATE
        from agents.house_prompt import _HOUSE_SYSTEM_TEMPLATE

        subs = [
            {
                "name": "Dev",
                "description": "Developer — pure implementer: receives specs and tasks from the orchestrator, writes clean tested TDD code, handles errors",
                "system_prompt": _DEV_SYSTEM_TEMPLATE,
                "tools": list(get_native_tool_names("developer")),
                "model": "deepseek:v4.1-flash",
            },
            {
                "name": "General",
                "description": "General — non-coding life & work assistant: research, documents (PDF/docx/xlsx), planning, web browsing, image understanding, personal admin. NOT a coder — code requests route back to Dev.",
                "system_prompt": _GENERAL_SYSTEM_TEMPLATE,
                "tools": list(get_native_tool_names("general")),
                "model": "deepseek:v4.1-flash",
            },
            {
                "name": "House",
                "description": "House — home control specialist: reads and actuates the smart home via house_* tools (lights, climate, locks, plugs, blinds, cameras, scenes). NOT a coder — code requests route back to Dev.",
                "system_prompt": _HOUSE_SYSTEM_TEMPLATE,
                "tools": list(get_native_tool_names("house")),
                "model": "deepseek:v4.1-flash",
            },
        ]
        for s in subs:
            # CF1: deterministic tool order — `set()` iteration is hash-randomized
            # across processes, which would make each subagent SystemMessage bytes
            # differ per restart/subprocess and silently invalidate the DeepSeek
            # prefix cache. dict.fromkeys preserves first-occurrence order.
            # D2: terminal-node subagents (Dev/General) get NO
            # delegation tools — every prompt declares them terminal nodes and the
            # orchestrator is the only delegator. general-purpose keeps them below.
            s["tools"] = list(dict.fromkeys(s.get("tools", [])))
            _SUBAGENTS_REGISTRY[s["name"]] = s

        # Register explicit aliases for backward compatibility and tests.
        # FreeAgent/general-purpose now resolve to the General subagent; its own
        # node runs the loop, so these names only affect prompt/tool resolution.
        aliases = {
            "Vision": "General",
            "FreeAgent": "General",
            "general-purpose": "General",
        }
        for alias, base_name in aliases.items():
            if base_name in _SUBAGENTS_REGISTRY:
                alias_spec = dict(_SUBAGENTS_REGISTRY[base_name])
                alias_spec["name"] = alias
                _SUBAGENTS_REGISTRY[alias] = alias_spec

        # True subagents (task-scoped specialists) from agents/specialists/*.json.
        # Workspace agents above stay code-defined; specialists are config-driven
        # (opencode-style). Missing/corrupt file is a silent no-op so the three
        # workspace agents always come up.
        for spec in _load_subagents_config():
            _SUBAGENTS_REGISTRY[spec["name"]] = spec
    except Exception as e:
        print(f"Error loading modularized subagents: {e}")


def _specialists_dir() -> str:
    """Directory for per-specialist JSON files (agents/specialists/*.json)."""
    return os.path.join(
        os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
        "agents", "specialists",
    )


def _parse_specialist_entry(e: dict) -> dict | None:
    """Normalize one raw JSON dict into a registry spec."""
    if not isinstance(e, dict):
        return None
    name = str(e.get("name") or "").strip()
    prompt = e.get("system_prompt")
    if isinstance(prompt, list):
        prompt = "\n".join(str(ln) for ln in prompt)
    elif not isinstance(prompt, str):
        prompt = ""
    when_to_use = str(e.get("when_to_use") or "").strip()
    tools = [str(t) for t in (e.get("tools") or [])]
    if os.environ.get("DEEP_AGENTS_EVAL_RUN") == "1":
        # Headless eval: no human to answer ask_user (it would block to its
        # timeout), so drop the interactive tools from every specialist spec.
        tools = [t for t in tools if t not in ("ask_user", "propose_plan")]
    skills = [str(s).strip() for s in (e.get("skills") or []) if str(s).strip()]
    internal = bool(e.get("internal"))
    spec = {
        "name": name,
        "kind": "subagent",
        "internal": internal,
        "description": when_to_use or "Specialist subagent.",
        "when_to_use": when_to_use,
        "system_prompt": prompt or "You are a specialized subagent.",
        "tools": list(dict.fromkeys(tools)),
        "skills": list(dict.fromkeys(skills)),
        "backend": str(e.get("backend") or "").strip().lower(),
    }
    if e.get("model"):
        spec["model"] = str(e["model"])
    if not spec["name"]:
        return None
    # Internal specialists (Compactor) may have tools=[]; others need ≥1 tool.
    if not spec["tools"] and not internal:
        return None
    return spec


def _load_subagents_config() -> list[dict]:
    """Load specialist specs from agents/specialists/*.json. Never raises."""
    try:
        import glob as _glob
        import json as _json

        out: list[dict] = []
        spec_dir = _specialists_dir()
        if os.path.isdir(spec_dir):
            for path in sorted(_glob.glob(os.path.join(spec_dir, "*.json"))):
                try:
                    with open(path, "r", encoding="utf-8") as f:
                        data = _json.load(f)
                    # Per-file format is a single spec dict; allow wrapped list too.
                    entries: list[dict]
                    if isinstance(data, dict) and "subagents" in data:
                        entries = data.get("subagents") or []
                    elif isinstance(data, dict) and data.get("name"):
                        entries = [data]
                    elif isinstance(data, list):
                        entries = data
                    else:
                        continue
                    for e in entries:
                        spec = _parse_specialist_entry(e)
                        if spec:
                            out.append(spec)
                except Exception as e:
                    print(f"[SUBAGENT] {os.path.basename(path)} load skipped: {e}")
            if out:
                return out

        return out
    except Exception as e:
        print(f"[SUBAGENT] specialists load skipped: {e}")
        return []


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
        if name not in allowed:
            return f"Error: Unknown subagent '{name}'. Available: {', '.join(allowed)}"
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
    # UI design parity: the UI specialist rides the Dev tier with full
    # thinking — design taste is the whole point, so effort defaults to high.
    if not reasoning_effort and str(name or "").strip().lower() in (
        "ui", "ui-design", "design",
    ):
        reasoning_effort = "high"

    if _specialist_backend(name) == "free":
        # Free pool with live streaming to the parent chat bus; one bounded
        # non-streaming retry on mid-stream failure. Returns the canonical
        # (text, tool_calls, reasoning) triple. Raises loudly on total pool
        # failure — no silent paid fallback by design.
        from llm.free_providers import free_native_turn

        return free_native_turn(
            name,
            messages,
            tools=native_tools or None,
            capability="coding",
            temperature=temp,
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
    return run_native_agent(
        role=name,
        messages=list(messages),
        tools=native_tools,
        temp=temp,
        reasoning_effort=reasoning_effort,  # static per-role mapping applies downstream
        where=where,
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
        execute_tool_detailed,
        shared_state,
    )
    depth = _TASK_DEPTH.get()
    if depth >= _MAX_TASK_DEPTH:
        return f"Error: Maximum task depth ({_MAX_TASK_DEPTH}) reached. Cannot spawn subagent '{name}' at depth {depth}."

    if not _SUBAGENTS_REGISTRY:
        _load_subagents()

    if name not in _SUBAGENTS_REGISTRY:
        allowed = ", ".join(_SUBAGENTS_REGISTRY.keys())
        return f"Error: Unknown subagent '{name}'. Available subagents: {allowed}"

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
    try:
        chat_id = shared_state.get("chat_id", "default_chat")
        if name == "Dev":
            from agents.developer_agent import developer_node
            from sync.state_sync import safe_update_state
            
            project_path = shared_state.get("project_path", _HARNESS_ROOT)
            
            dev_state = {
                "client_request": task,
                "tech_spec": shared_state.get("outputs", {}).get("tech_spec", ""),
                "requirements": shared_state.get("outputs", {}).get("requirements", ""),
                "project_path": project_path,
                "chat_id": chat_id,
                "error_count": 0,
                "test_report": "",
            }
            
            res_dict = developer_node(dev_state)
            
            # Sync outputs back to shared_state
            if "code" in res_dict and res_dict["code"]:
                safe_update_state({
                    "outputs": {
                        **shared_state.get("outputs", {}),
                        "code": res_dict["code"],
                        "test_report": res_dict.get("test_report", ""),
                    }
                })
            
            return _handoff(res_dict.get("agent_report") or res_dict.get("code") or "Developer completed execution.", "Dev")

        # General routes through its own node (agents/general_agent.py) —
        # dual-backend loop (deepseek flash or free providers), profile and
        # chat-history injection, session logging. The old FreeAgent
        # single-shot special case is gone: task('FreeAgent') now resolves to
        # the General spec via the registry alias.
        if name in ("General", "general-purpose", "FreeAgent"):
            from agents.general_agent import general_node
            from sync.state_sync import safe_update_state

            gen_state = {
                "client_request": task,
                "project_path": shared_state.get("project_path", _HARNESS_ROOT),
                "chat_id": chat_id,
            }
            res_dict = general_node(gen_state)
            return _handoff(res_dict.get("agent_report") or "General completed with no output.", name)

        # House routes through its own node (agents/house_agent.py) —
        # home-control loop over the house_* tools, same shape as General.
        if name == "House":
            from agents.house_agent import house_node
            from sync.state_sync import safe_update_state

            house_state = {
                "client_request": task,
                "project_path": shared_state.get("project_path", _HARNESS_ROOT),
                "chat_id": chat_id,
            }
            res_dict = house_node(house_state)
            return _handoff(res_dict.get("agent_report") or "House completed with no output.", name)

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
            ToolReturnPart,
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
        content = ""
        _sub_trouble = False
        _last_tool_name = ""
        try:
            sub_iter = 0
            while True:
                sub_iter += 1
                # Call LLM
                _effort = subagent_effort
                res = _specialist_call_backend(
                    name,
                    list(messages),
                    native_tools=native_tools,
                    schema=schema,
                    model=subagent_model,
                    reasoning_effort=_effort,
                    where=f"{name} iter {sub_iter}: {_last_tool_name or 'start'}",
                )

                # Extract response text
                additional_kwargs = {}
                reasoning = getattr(res, "reasoning_content", None)
                if reasoning:
                    additional_kwargs["reasoning_content"] = reasoning

                # A5: native full turn — pydantic-ai executed the tools already.
                # The slice (ModelResponse tool-calls + ModelRequest tool results)
                # IS the execution record; extend history, log to the session, and
                # finish when the model stopped calling tools.
                _native_slice = None
                if isinstance(res, tuple) and len(res) == 3 and _is_native_slice(res[1]):
                    response_text, _native_slice, reasoning = res
                    content = str(response_text or "")
                    if reasoning:
                        additional_kwargs["reasoning_content"] = reasoning
                    messages.extend(_native_slice)
                    _native_calls = [
                        {"id": p.tool_call_id or "", "name": p.tool_name or ""}
                        for m in _native_slice
                        if isinstance(m, ModelResponse)
                        for p in getattr(m, "parts", []) or []
                        if isinstance(p, ToolCallPart)
                    ]
                    if sub_sess is not None:
                        try:
                            blocks = [{"type": "text", "text": content}]
                            if reasoning:
                                blocks.insert(0, {"type": "thinking", "thinking": reasoning})
                            stop = "toolUse" if _native_calls else "stop"
                            sub_sess.append_assistant(blocks, stop_reason=stop)
                            for tc in _native_calls:
                                sub_sess.append_tool_result(
                                    "ok",
                                    tool_call_id=tc.get("id", ""),
                                    tool_name=tc.get("name", ""),
                                    is_error=False,
                                )
                        except Exception:
                            pass
                    if _native_calls:
                        # Native turn used tools; continue the loop so a final
                        # no-tool turn produces the handoff.
                        _last_tool_name = _native_calls[-1].get("name", "")
                        continue
                    break

                if schema and hasattr(res, "model_dump_json"):
                    content = res.model_dump_json()
                    messages.append(ModelResponse(parts=[TextPart(content=content)]))
                    if sub_sess is not None:
                        try:
                            sub_sess.append_assistant(content, stop_reason="stop")
                        except Exception:
                            pass
                    break  # Structured output â†’ done
                elif schema and hasattr(res, "json"):
                    content = res.json()
                    messages.append(ModelResponse(parts=[TextPart(content=content)]))
                    if sub_sess is not None:
                        try:
                            sub_sess.append_assistant(content, stop_reason="stop")
                        except Exception:
                            pass
                    break
                else:
                    # Native mode returns (text, native_tool_calls, reasoning)
                    native_tool_calls = []
                    if isinstance(res, tuple):
                        if len(res) == 3:
                            response_text, native_tool_calls, reasoning = res
                        else:
                            response_text, native_tool_calls = res
                            reasoning = None
                        if reasoning:
                            additional_kwargs["reasoning_content"] = reasoning
                    else:
                        response_text = str(res)
                    content = response_text

                # Native tool calls only. Tool schemas and call arguments stay
                # in the provider's structured API protocol.
                tool_calls = []
                for call_index, tc in enumerate(native_tool_calls or []):
                    tname = tc.get("tool") or tc.get("name", "")
                    if tname:
                        tool_calls.append({
                            "tool": tname,
                            "args": tc.get("args") or {},
                            "id": tc.get("id") or f"sub-{sub_iter}-{call_index + 1}",
                        })

                if not tool_calls:
                    # No native tool calls: the subagent chose its final response.
                    parts: list = []
                    if additional_kwargs.get("reasoning_content"):
                        parts.append(ThinkingPart(content=additional_kwargs["reasoning_content"]))
                    parts.append(TextPart(content=content))
                    messages.append(ModelResponse(parts=parts))
                    if sub_sess is not None:
                        try:
                            blocks = [{"type": "text", "text": content}]
                            if additional_kwargs.get("reasoning_content"):
                                blocks.insert(0, {"type": "thinking",
                                                  "thinking": additional_kwargs["reasoning_content"]})
                            sub_sess.append_assistant(blocks, stop_reason="stop")
                        except Exception:
                            pass
                    break

                # Record the AI response (native: carry tool-call parts)
                parts: list = []
                if additional_kwargs.get("reasoning_content"):
                    parts.append(ThinkingPart(content=additional_kwargs["reasoning_content"]))
                if content:
                    parts.append(TextPart(content=content))
                for tc in tool_calls:
                    parts.append(ToolCallPart(
                        tool_name=tc["tool"],
                        args=tc.get("args") or {},
                        tool_call_id=tc.get("id", ""),
                    ))
                messages.append(ModelResponse(parts=parts))

                # Child session log (P1): record the step's assistant message
                if sub_sess is not None:
                    try:
                        blocks = [{"type": "text", "text": content}]
                        if reasoning:
                            blocks.insert(0, {"type": "thinking", "thinking": reasoning})
                        sub_sess.append_assistant(blocks, stop_reason="toolUse")
                    except Exception:
                        pass

                # Execute tools and collect results. Independent read-only calls
                # (PARALLEL_SAFE) run concurrently; results stay in call order so
                # the provider's call-id → result pairing is preserved.
                # Out-of-scope tools are filtered BEFORE execution.
                tool_results = []
                structured_results = []
                _sub_exec_pool = None
                _sub_allowed = [c for c in tool_calls if _tool_in_scope(c.get("tool", ""), tool_names)]
                try:
                    from tools import run_tool_calls as _run_tool_calls
                    _sub_exec_pool = _run_tool_calls(
                        _sub_allowed,
                        lambda tc: _subagent_execute_one(tc, tool_names),
                    )
                except Exception:
                    _sub_exec_pool = []
                _sub_result_by_id = {}
                if len(_sub_exec_pool) == len(_sub_allowed):
                    for _tc, _res in zip(_sub_allowed, _sub_exec_pool):
                        _sub_result_by_id[str(_tc.get("id") or "")] = _res
                for tc in tool_calls:
                    tool_name = tc.get("tool", "")
                    args = tc.get("args", {})
                    _cid = str(tc.get("id") or "")
                    if _cid in _sub_result_by_id:
                        result = _sub_result_by_id[_cid]
                    elif not _tool_in_scope(tool_name, tool_names):
                        result = f"[SKIPPED] Tool '{tool_name}' not in subagent permissions."
                    else:
                        try:
                            execution = execute_tool_detailed(tool_name, args)
                            result = execution.content
                        except Exception as e:
                            result = f"ERROR: {str(e)[:200]}"

                    result = str(result)
                    tool_results.append(f"[{tool_name}] {result}")
                    structured_results.append((tc, result))
                    if sub_sess is not None:
                        try:
                            is_error = str(result).lstrip().startswith(("ERROR:", "[SKIPPED]", "Error:"))
                            sub_sess.append_tool_result(
                                result,
                                tool_call_id=tc["id"],
                                tool_name=tool_name,
                                is_error=is_error,
                            )
                        except Exception:
                            pass
                    # Verification-before-DONE bookkeeping
                    _last_tool_name = tool_name

                # Feed every result back as a native ToolMessage. Never merge
                # tool output into a synthetic human turn; the provider needs
                # the call id to associate each result with its function call.
                feedback = "\n".join(tool_results) if tool_results else "No tool results."
                # Trouble signal → the NEXT turn diagnoses with the spec effort
                _j = feedback.lower()
                if any(k in _j for k in ("[error]", "[skipped]", "failed", "exception",
                                         "traceback", "timeout", "not found", "cannot find module")):
                    _sub_trouble = True
                for tc, result in structured_results:
                    messages.append(ModelRequest(parts=[ToolReturnPart(
                        tool_name=str(tc.get("tool", "")),
                        content=str(result),
                        tool_call_id=str(tc["id"]),
                    )]))

                # Vision: drain tool-queued inline images into ONE user message.
                try:
                    from llm.deps import WorkspaceDeps
                    _sub_deps = WorkspaceDeps.from_shared_state(role=name, chat_id=chat_id)
                    from tools.vision_channel import drain_into_messages_native
                    drain_into_messages_native(messages, header="[Inline image(s) from tool result]", ctx=_sub_deps)
                except Exception:
                    pass

            # No budget checks here; subagents are managed by LoopGuard and structural loops.

            # [Security & performance are enforced by the tiered CI gate on push
            #  (fast-checks / staging / release-heavy) — the gate replaces the former DevSecOps
            #  host-validation step.]

        except Exception:
            raise

    finally:
        active_permissions.reset(token)
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

def _is_native_slice(second: list) -> bool:
    """True when a backend result's 2nd element is a transcript slice
    (native full turn) rather than a list of tool-call dicts (free
    single-turn triple)."""
    try:
        from pydantic_ai.messages import ModelRequest as _MR, ModelResponse as _MS
        for m in (second or [])[:2]:
            if isinstance(m, (_MR, _MS)):
                return True
            if isinstance(m, dict) and ("tool" in m or "name" in m):
                return False
        return False
    except Exception:
        return False


def _subagent_execute_one(tool_call: dict, tool_names: list) -> str:
    """Thread-pool worker for a subagent's parallel read-only batch. Only
    PARALLEL_SAFE tools reach this path (enforced by run_tool_calls)."""
    tool_name = str(tool_call.get("tool", "") or "")
    args = tool_call.get("args") or {}
    if not isinstance(args, dict):
        args = {}
    if not _tool_in_scope(tool_name, tool_names):
        return f"[SKIPPED] Tool '{tool_name}' not in subagent permissions."
    try:
        from tools import execute_tool_detailed
        execution = execute_tool_detailed(tool_name, args)
        return str(execution.content)
    except Exception as e:
        return f"ERROR: {str(e)[:200]}"


# Alias captured after task() is defined; call_subagent's `task` parameter
# would otherwise shadow the module-level function at call time.
_TASK_TOOL_DELEGATE = task

