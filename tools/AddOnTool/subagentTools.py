"""AddOnTool subagent and asynchronous task orchestration.

The facade re-exports these names. Shared runtime, registry, and workspace state
are imported lazily inside functions so the package can load without a cycle.
"""
import collections
import contextvars
import json
import os
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from typing import Optional

try:
    from deepagents.middleware.filesystem import FilesystemPermission
except ImportError:
    class FilesystemPermission:  # type: ignore[no-redef]
        def __init__(self, operations=None, paths=None, mode="allow"):
            self.operations = operations or []
            self.paths = paths or []
            self.mode = mode
from langchain_core.messages import SystemMessage, HumanMessage, AIMessage, ToolMessage
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

_ASYNC_TASKS: dict[str, dict] = {}

_ASYNC_LOCK = threading.Lock()

_ASYNC_EXECUTOR = ThreadPoolExecutor(max_workers=20)

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
                "model": "deepseek:v4-flash",
            },
            {
                "name": "General",
                "description": "General — non-coding life & work assistant: research, documents (PDF/docx/xlsx), planning, web browsing, image understanding, personal admin. NOT a coder — code requests route back to Dev.",
                "system_prompt": _GENERAL_SYSTEM_TEMPLATE,
                "tools": list(get_native_tool_names("general")),
                "model": "deepseek:v4-flash",
            },
            {
                "name": "House",
                "description": "House — home control specialist: reads and actuates the smart home via house_* tools (lights, climate, locks, plugs, blinds, cameras, scenes). NOT a coder — code requests route back to Dev.",
                "system_prompt": _HOUSE_SYSTEM_TEMPLATE,
                "tools": list(get_native_tool_names("house")),
                "model": "deepseek:v4-flash",
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


def _subagents_config_path() -> str:
    """Legacy monolith path — kept as fallback until all clones migrate."""
    return os.path.join(
        os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
        "agents", "subagents.json",
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
    internal = bool(e.get("internal"))
    spec = {
        "name": name,
        "kind": "subagent",
        "internal": internal,
        "description": when_to_use or "Specialist subagent.",
        "when_to_use": when_to_use,
        "system_prompt": prompt or "You are a specialized subagent.",
        "tools": list(dict.fromkeys(tools)),
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
    """Load specialist specs from agents/specialists/*.json, falling back to
    the legacy monolith agents/subagents.json. Never raises.
    """
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

        # Legacy fallback.
        legacy = _subagents_config_path()
        if not os.path.isfile(legacy):
            return out
        with open(legacy, "r", encoding="utf-8") as f:
            data = _json.load(f)
        entries = data.get("subagents") if isinstance(data, dict) else None
        if not isinstance(entries, list):
            return out
        for e in entries:
            spec = _parse_specialist_entry(e)
            if spec:
                out.append(spec)
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


def call_subagent(name: str = "", task: str = "") -> str:
    """Spawn a specialist subagent and block until it finishes.

    The subagent's final assistant message is returned verbatim as the tool
    result (opencode/Claude-Code style); oversized output gets the standard
    VFS offload pointer. Only true subagents from agents/specialists/*.json are
    spawnable here; workspace-agent delegation stays on the task tool.
    """
    if not name or not task:
        return "Error: call_subagent requires name and task."
    allowed = _subagent_only_names()
    if name not in allowed:
        return f"Error: Unknown subagent '{name}'. Available: {', '.join(allowed)}"
    return _TASK_TOOL_DELEGATE(name=name, task=task)


def _specialist_backend(name: str) -> str:
    """Resolve the LLM backend for a specialist subagent.

    Precedence: SUBAGENT_MODEL_BACKEND env > active provider mode's
    specialist entry > agents/specialists/*.json spec "backend" > "deepseek".
    Only "free" dispatches to the free pool; every other value rides the paid
    llm.py path (which itself resolves deepseek | opencode | command).
    """
    if not _SUBAGENTS_REGISTRY:
        _load_subagents()
    spec_backend = str((_SUBAGENTS_REGISTRY.get(name) or {}).get("backend") or "").strip().lower()
    try:
        from llm.provider_modes import specialist_backend as _mode_backend

        return _mode_backend(spec_backend)
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
        # Provider modes may pin a concrete specialist model (command mode ->
        # laguna); canonical names map through llm.py's provider slots.
        try:
            from llm.provider_modes import specialist_model

            model = specialist_model()
        except Exception:
            model = None

    return run_native_agent(
        role=name,
        messages=list(messages),
        tools=native_tools,
        temp=temp,
        reasoning_effort=reasoning_effort,
        where=where,
        system_prompt=_child_prompt_for(name),
    )

_HANDOFF_MAX_CHARS = 1000  # 1000 chars — compact structured report; oversized text offloads to VFS

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

def _compact_handoff(content, name: str = "subagent", vfs_path: str = "") -> str:
    """Compress a subagent's final message into the AGENT HANDOFF REPORT format.

    Subagents report back to another LLM (the orchestrator), not a human. The report
    only needs: status, files, error classification, failure locus, and a minimal
    error trail. No prose, no full tracebacks, no code dumps (the orchestrator
    re-delegates with the gap; it never fixes from a report).
    Hard cap: 1000 chars. Full raw text offloads to VFS with a pointer line.

    Strategy: (1) pass through the model's own report if it followed the format;
    (2) lenient fallback â€” build the report from the raw output via heuristics.
    """
    import re as _re

    content = str(content or "").strip()
    if not content:
        content = "Status: DONE - no output produced."

    # 1) Model followed the report format -> normalize + cap
    if "AGENT HANDOFF REPORT" in content:
        idx = content.find("### AGENT HANDOFF REPORT")
        report = content[idx:].strip()
        # cut trailing prose after the final code fence
        fence = report.rfind("```")
        if fence != -1 and len(report) - fence < _HANDOFF_MAX_CHARS:
            report = report[:fence + 3]
        return report[:_HANDOFF_MAX_CHARS]

    # 2) Heuristic build from raw output
    low = content.lower()
    if any(k in low for k in ("unable", "stuck", "error", "failed", "exception", "traceback", "exit code 1", "did not pass", "tests failed")):
        status = "FAILED"
    elif any(k in low for k in ("blocked", "cannot", "waiting", "needs")):
        status = "BLOCKED"
    else:
        status = "DONE"

    lines = [l.strip() for l in content.splitlines() if l.strip()]
    tail = "\n".join(lines[-5:])

    # Error classification
    m_exc = _re.search(r"(\w+(?:Error|Exception|Failure|Warning|Timeout))", content)
    exc = m_exc.group(1) if m_exc else ""
    m_cat = _re.search(r"Error Category[:\s]*`?([^`\n]+)", content, _re.IGNORECASE)
    category = m_cat.group(1).strip() if m_cat else (_classify_error(exc) if exc else "")
    m_loc = _re.search(r"([\w./\\-]+\.(?:py|js|ts|php|go|rs|java|rb))[::](\d+)", content)
    loc = f"{m_loc.group(1)} (Line {m_loc.group(2)})" if m_loc else ""
    m_fn = _re.search(r"\bin\s+(\w+)\s*\(", content) or _re.search(r"\bdef\s+(\w+)\s*\(", content)
    fn = (m_fn.group(1) + "()") if m_fn else ""
    m_cmd = _re.search(r"(?:^|\n)[^\n]*(pytest|npm (?:test|run|install)|node |python\s+|go test|make |npx )", content)
    cmd = m_cmd.group(0).strip()[:120] if m_cmd else ""

    # Files touched
    files = []
    for l in lines:
        cf = l.lstrip("-*[] ").strip().rstrip(",")
        if any(c in cf for c in ("/", "\\", ".")) and len(cf) < 200 and not cf.startswith(("PASS", "FAIL", "Traceback", "File ")):
            files.append(cf)
        if len(files) >= 10:
            break

    # Trigger: prefer a line with expected/should/when, else a short leading line
    trigger = ""
    for l in lines:
        if _re.search(r"\b(trigger|expected|should|when|because)\b", l, _re.IGNORECASE) and len(l) < 240:
            trigger = l
            break
    if not trigger:
        first = lines[0] if lines else ""
        if len(first) < 240:
            trigger = first

    vfs_line = f"- **VFS Log Path:** `{vfs_path}` (Full log offloaded)\n" if vfs_path else ""
    if status == "DONE":
        out = ["### AGENT HANDOFF REPORT [DONE]",
               f"- **Agent:** {name} | **Status:** DONE",
               vfs_line.rstrip("\n"),
               f"- **Files:** {', '.join('`%s`' % f for f in files[:10]) if files else '(none)'}",
               "",
               "#### 1. OUTCOME",
               f"- {trigger or 'Completed.'}"]
    else:
        out = [f"### AGENT HANDOFF REPORT [{status}]",
               f"- **Agent:** {name} | **Status:** {status}",
               vfs_line.rstrip("\n"),
               "",
               "#### 1. ERROR CLASSIFICATION",
               f"- **Error Category:** `{category}`" if category else "- **Error Category:** UNKNOWN",
               f"- **Primary Exception:** `{exc}`" + (f" in `{loc}`" if loc else "") if exc else "",
               f"- **Failing Command:** `{cmd}`" if cmd else "",
               "",
               "#### 2. FAILURE LOCUS & CONTEXT",
               f"- **File:** `{loc}`" if loc else "- **File:** (not identified)",
               f"- **Function:** `{fn}`" if fn else "",
               f"- **Trigger:** {trigger}" if trigger else "",
               "",
               "#### 3. MINIMAL ERROR TRAIL (Tail 5 Lines)",
               "```text",
               tail,
               "```",
               "",
               f"- **Files:** {', '.join('`%s`' % f for f in files[:10]) if files else '(none)'}",
               f"- **Next:** re-delegate to `Dev`" if status == "FAILED" else ""]
    report = "\n".join(x for x in out if x)
    return report[:_HANDOFF_MAX_CHARS]

def _handoff(content, name: str = "subagent") -> str:
    """Full handoff pipeline: offload raw output to VFS when it exceeds the cap,
    then produce the compact AGENT HANDOFF REPORT with a pointer to the full log."""
    from tools import _write_file
    import uuid
    content = str(content or "").strip()
    vfs_path = ""
    if len(content) > _HANDOFF_MAX_CHARS:
        try:
            file_id = uuid.uuid4().hex[:8]
            clean = "".join(c if c.isalnum() else "_" for c in name)[:40] or "subagent"
            vfs_path = f"/scratch/handoff_log_{clean}_{file_id}.txt"
            _write_file(file_path=vfs_path, content=content)
        except Exception:
            vfs_path = ""
    return _compact_handoff(content, name, vfs_path)

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
        _tool_cap,
        active_permissions,
        auto_offload_result,
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
            native_tools = [t for t in get_native_tools()
                            if t.get("function", {}).get("name") in tool_names]
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
            
            # Orchestrator dial: an explicit reasoning_effort override sets Dev's
            # planning depth for THIS delegation (default stays REASONING_ITER1).
            _prev_iter1 = os.environ.get("REASONING_ITER1")
            if reasoning_effort:
                os.environ["REASONING_ITER1"] = reasoning_effort
            try:
                res_dict = developer_node(dev_state)
            finally:
                if _prev_iter1 is not None:
                    os.environ["REASONING_ITER1"] = _prev_iter1
                else:
                    os.environ.pop("REASONING_ITER1", None)
            
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

        messages = [
            SystemMessage(content=full_sys_prompt),
            HumanMessage(content=resolved_task),
        ]

        # Vision: inject user-attached images stashed by the API layer.
        try:
            from tools.vision_channel import drain_attachment_images
            drain_attachment_images(messages)
        except Exception:
            pass

        # ── Child session log (P1): fresh child per delegation, lineage to the
        # orchestrator's session (best-effort; never breaks the loop) ──
        sub_sess = None
        try:
            from uuid import uuid4
            from session_log import get_session, find_recent_session
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

        from llm import invoke_messages_with_fallback, invoke_messages_with_fallback_stream

        # Orchestrator dial overrides the registry default for this delegation
        # (registry keeps the spec effort when no override is given).
        subagent_effort = reasoning_effort or spec.get("reasoning_effort")

        # ── Subagent loop: LLM → tools → observe → repeat ──
        # There is no turn ceiling. Cancellation, provider failure, a final
        # response, or an explicit structured-output request are the lifecycle
        # boundaries; context checkpoints handle long transcripts.
        content = ""
        # Adaptive reasoning ladder (same pattern as Dev/Orchestrator):
        # iter-1 (diagnosis) uses the spec effort; routine turns run disabled;
        # a trouble signal (tool error / failure markers) flips the NEXT turn
        # back to the spec effort for diagnosis.
        _sub_trouble = False
        _routine_effort = os.environ.get("REASONING_SUBAGENT_ROUTINE", "disabled")
        _last_tool_name = ""
        try:
            sub_iter = 0
            while True:
                sub_iter += 1
                # Call LLM
                if sub_iter == 1 or _sub_trouble:
                    _effort = subagent_effort
                    _sub_trouble = False
                else:
                    _effort = _routine_effort
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
                # The slice (AIMessage tool_calls + ToolMessage results) IS the
                # execution record; extend history, log to the session, and
                # finish when the model stopped calling tools.
                _native_slice = None
                if isinstance(res, tuple) and len(res) == 3 and _is_native_slice(res[1]):
                    response_text, _native_slice, reasoning = res
                    content = str(response_text or "")
                    if reasoning:
                        additional_kwargs["reasoning_content"] = reasoning
                    messages.extend(_native_slice)
                    _native_calls = [
                        tc for m in _native_slice
                        if m.__class__.__name__ == "AIMessage"
                        for tc in (getattr(m, "tool_calls", None) or [])
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
                    messages.append(AIMessage(content=content, additional_kwargs=additional_kwargs))
                    if sub_sess is not None:
                        try:
                            sub_sess.append_assistant(content, stop_reason="stop")
                        except Exception:
                            pass
                    break  # Structured output â†’ done
                elif schema and hasattr(res, "json"):
                    content = res.json()
                    messages.append(AIMessage(content=content, additional_kwargs=additional_kwargs))
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
                    messages.append(AIMessage(content=content, additional_kwargs=additional_kwargs))
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

                # Record the AI response (native: carry tool_calls for the API round-trip)
                if native_tool_calls and tool_calls:
                    messages.append(AIMessage(
                        content=content,
                        tool_calls=[{"name": tc["tool"], "args": tc.get("args") or {},
                                     "id": tc.get("id", "")} for tc in tool_calls],
                        additional_kwargs=additional_kwargs,
                    ))
                else:
                    messages.append(AIMessage(content=content, additional_kwargs=additional_kwargs))

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
                _sub_allowed = [c for c in tool_calls if c.get("tool") in tool_names or not tool_names]
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
                    elif tool_name not in tool_names and tool_names:
                        result = f"[SKIPPED] Tool '{tool_name}' not in subagent permissions."
                    else:
                        try:
                            execution = execute_tool_detailed(tool_name, args)
                            result = execution.content
                        except Exception as e:
                            result = f"ERROR: {str(e)[:200]}"

                    result = auto_offload_result(str(result), tool_name, max_chars=_tool_cap("subagent"))
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
                    messages.append(ToolMessage(
                        content=str(result),
                        tool_call_id=tc["id"],
                        name=tc.get("tool", ""),
                    ))

                # Vision: drain tool-queued inline images into ONE user message.
                try:
                    from tools.vision_channel import drain_into_messages
                    drain_into_messages(messages, header="[Inline image(s) from tool result]")
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
    # the tool result — verbatim, opencode/Claude-Code style. Only oversized
    # text gets the harness-standard VFS offload pointer. Workspace agents
    # above keep the structured AGENT HANDOFF contract their consumer parses.
    if spec.get("kind") == "subagent":
        return auto_offload_result(content, "subagent", max_chars=_tool_cap("subagent"))

    # Full output beyond the handoff cap is kept on disk (VFS) with a pointer in the
    # report; the handoff itself stays compact so the orchestrator's context never
    # eats a multi-KB dump.
    return _handoff(content, name)

def _is_native_slice(second: list) -> bool:
    """True when a backend result's 2nd element is a LangChain transcript
    slice (native full turn) rather than a list of tool-call dicts (free
    single-turn triple)."""
    try:
        for m in (second or [])[:2]:
            if hasattr(m, "__class__") and m.__class__.__name__ in ("AIMessage", "ToolMessage"):
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
    if tool_name not in tool_names and tool_names:
        return f"[SKIPPED] Tool '{tool_name}' not in subagent permissions."
    try:
        execution = execute_tool_detailed(tool_name, args)
        return str(execution.content)
    except Exception as e:
        return f"ERROR: {str(e)[:200]}"


def _run_subagent_in_thread(task_id: str, name: str, task_desc: str, depth: int) -> None:
    """Run a subagent in a background thread. Sets result/error on _ASYNC_TASKS.

    P6: async spawns are continuable — a durable child session is created and
    its child_id is recorded on the task so send_message() can follow up.
    """
    from tools import _HARNESS_ROOT
    try:
        _TASK_DEPTH.set(depth)
        from agents.subagent_manager import start_continuable
        from session_log import find_recent_session
        from sync.state_sync import shared_state as _ss
        ws = _ss.get("project_path") or _HARNESS_ROOT
        chat_id = str(_ss.get("chat_id") or "")
        parent_path = None
        try:
            parent_path = find_recent_session(chat_id, ws)
        except Exception:
            parent_path = None
        r = start_continuable(chat_id, ws, name, task_desc, parent_session=parent_path)
        result = r.get("result", "")
        child_id = r.get("child_id", "")
        # Parity with the sync path: specialist results pass through verbatim
        # (VFS offload only when oversized); workspace agents stay raw here.
        try:
            if not _SUBAGENTS_REGISTRY:
                _load_subagents()
            if (_SUBAGENTS_REGISTRY.get(name) or {}).get("kind") == "subagent":
                from tools import _tool_cap as _tc, auto_offload_result as _aor
                result = _aor(result, "subagent", max_chars=_tc("subagent"))
        except Exception:
            pass
        with _ASYNC_LOCK:
            if task_id in _ASYNC_TASKS:
                _ASYNC_TASKS[task_id]["status"] = "completed"
                _ASYNC_TASKS[task_id]["result"] = result
                _ASYNC_TASKS[task_id]["child_id"] = child_id
    except Exception as e:
        with _ASYNC_LOCK:
            if task_id in _ASYNC_TASKS:
                _ASYNC_TASKS[task_id]["status"] = "failed"
                _ASYNC_TASKS[task_id]["error"] = str(e)[:500]

def start_async_task(name: str, task: str) -> str:
    """Fire-and-forget subagent. Returns task_id immediately. Subagent runs in background.

    Use this to spawn MULTIPLE subagents in parallel. Poll with check_async_task().
    For immediate results, use the regular task() tool instead.

    Args:
        name: Registered subagent type (for example Dev or General)
        task: Detailed task description for the subagent
    """
    import uuid
    depth = _TASK_DEPTH.get()
    if depth >= _MAX_TASK_DEPTH:
        return f"Error: Maximum task depth ({_MAX_TASK_DEPTH}) reached. Cannot spawn subagent at depth {depth}."

    if not _SUBAGENTS_REGISTRY:
        _load_subagents()
    if name not in _SUBAGENTS_REGISTRY:
        allowed = ", ".join(_SUBAGENTS_REGISTRY.keys())[:200]
        return f"Error: Unknown subagent '{name}'. Available: {allowed}"

    task_id = f"{name}-{uuid.uuid4().hex[:6]}"
    with _ASYNC_LOCK:
        _ASYNC_TASKS[task_id] = {
            "status": "running",
            "name": name,
            "task": task,
            "depth": depth,
            "started_at": __import__('time').time(),
        }

    # Set depth for subagent's thread context
    child_depth = depth + 1
    _ASYNC_EXECUTOR.submit(_run_subagent_in_thread, task_id, name, task, child_depth)

    return f"Task started: {task_id}\nSubagent: {name}\nStatus: RUNNING"

def check_async_task(task_id: str, wait_seconds: int = 0) -> str:
    """Check the status of an async task. Returns result if completed.

    Args:
        task_id: The task ID returned by start_async_task()
        wait_seconds: If task is still running, wait up to this many seconds
                      before returning the status. Lets long tasks finish naturally
                      without polling. Max 300 seconds (5 minutes).
    """
    import time as time_mod
    wait_seconds = max(0, min(wait_seconds, 300))  # clamp 0-300
    with _ASYNC_LOCK:
        if task_id not in _ASYNC_TASKS:
            return f"Error: Unknown task '{task_id}'. Use list_async_tasks() to see active tasks."
        t = _ASYNC_TASKS[task_id]
        if t["status"] == "running":
            # If wait_seconds is set, sleep in small intervals and check again
            if wait_seconds > 0:
                _ASYNC_LOCK.release()
                try:
                    check_interval = 2  # check every 2 seconds
                    waited = 0
                    while waited < wait_seconds:
                        time_mod.sleep(min(check_interval, wait_seconds - waited))
                        waited += min(check_interval, wait_seconds - waited)
                        with _ASYNC_LOCK:
                            if task_id not in _ASYNC_TASKS:
                                return f"Error: Task '{task_id}' no longer exists."
                            t2 = _ASYNC_TASKS[task_id]
                            if t2["status"] != "running":
                                if t2["status"] == "completed":
                                    result = t2.get("result", "")
                                    child_id = t2.get("child_id", "")
                                    child_line = f"Child: {child_id}\n" if child_id else ""
                                    return f"STATUS: DONE\n{child_line}{result}"
                                else:
                                    return f"STATUS: FAILED\n{t2.get('error', 'Unknown error')}"
                finally:
                    _ASYNC_LOCK.acquire()
                # After waiting, re-read current state
                t = _ASYNC_TASKS.get(task_id)
                if t is None:
                    return f"Error: Task '{task_id}' no longer exists."
                if t["status"] == "running":
                    elapsed = time_mod.time() - t.get("started_at", 0)
                    return f"STATUS: RUNNING (waited {wait_seconds}s, elapsed: {elapsed:.0f}s)\nTask: {t['name']}: {t['task'][:150]}"
                elif t["status"] == "completed":
                    result = t.get("result", "")
                    child_id = t.get("child_id", "")
                    child_line = f"Child: {child_id}\n" if child_id else ""
                    return f"STATUS: DONE\n{child_line}{result}"
                else:
                    return f"STATUS: FAILED\n{t.get('error', 'Unknown error')}"
            else:
                elapsed = time_mod.time() - t.get("started_at", 0)
                return f"STATUS: RUNNING (elapsed: {elapsed:.0f}s)\nTask: {t['name']}: {t['task'][:150]}"
        elif t["status"] == "completed":
            result = t.get("result", "")
            child_id = t.get("child_id", "")
            child_line = f"Child: {child_id}\n" if child_id else ""
            return f"STATUS: DONE\n{child_line}{result}"
        else:
            return f"STATUS: FAILED\n{t.get('error', 'Unknown error')}"

def list_async_tasks() -> str:
    """List all async tasks and their statuses."""
    import time
    with _ASYNC_LOCK:
        if not _ASYNC_TASKS:
            return "No async tasks."

        # Auto-cleanup completed tasks older than 10 minutes
        now = time.time()
        to_remove = []
        for tid, t in _ASYNC_TASKS.items():
            if t["status"] in ("completed", "failed"):
                age = now - t.get("started_at", now)
                if age > 600:  # 10 minutes
                    to_remove.append(tid)
        for tid in to_remove:
            del _ASYNC_TASKS[tid]

        if not _ASYNC_TASKS:
            return "No async tasks."

        lines = [f"{len(_ASYNC_TASKS)} async tasks:"]
        for tid, t in sorted(_ASYNC_TASKS.items()):
            status_icon = {"running": "[>]", "completed": "[OK]", "failed": "[XX]"}.get(t["status"], "[??]")
            lines.append(f"  {status_icon} [{t['status'].upper()}] {tid}: {t['name']} â€” {t['task'][:100]}")
        return "\n".join(lines)


def async_task(action: str, name: str = "", task: str = "", task_id: str = "", wait_seconds: int = 0) -> str:
    """Unified async task manager — flexible single tool for orchestrator.

    action=start  → fire-and-forget (name, task required) → returns task_id
    action=status → poll one task (task_id, wait_seconds?) → returns DONE/FAILED/RUNNING
    action=list   → list all tasks → returns table

    Replaces start_async_task / check_async_task / list_async_tasks.
    Keeps old tools for Developer compatibility.
    """
    a = (action or "").strip().lower()
    if a in ("start", "create", "run"):
        if not name or not task:
            return "Error: async_task action=start requires name and task."
        return start_async_task(name, task)
    if a in ("status", "check", "poll", "get"):
        if not task_id:
            return "Error: async_task action=status requires task_id."
        try:
            ws = int(wait_seconds) if wait_seconds is not None else 0
        except Exception:
            ws = 0
        return check_async_task(task_id, ws)
    if a in ("list", "ls", "all"):
        return list_async_tasks()
    return "Error: async_task action must be one of: start, status, list. Got: '{}'".format(action)


# Alias captured after task() is defined; call_subagent's `task` parameter
# would otherwise shadow the module-level function at call time.
_TASK_TOOL_DELEGATE = task

