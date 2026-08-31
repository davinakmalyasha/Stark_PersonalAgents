"""
Subagent Manager â€” continuable subagents (dsh-style).

A **continuable child** is a durable child session (P1 JSONL) with:

- a ``subagent/descriptor`` custom entry (name, label, parent links)
- at most one live **Activation**: ``running`` (a turn owns the thread) or
  ``idle`` (quiescent, resumable)
- FIFO follow-up turns via :func:`send_message` â€” cold-resumes from the log
- :func:`interrupt_agent` â€” cooperative per-iteration cancel
- child â†’ parent :func:`report` â€” wakeup (follow-up to the parent chat) or
  quiet (log-only)
- durable enumeration via :func:`list_agents` â€” reads session headers, never
  resumes an agent

Follow-up authority: only the durable direct parent session (or the live
parent chat) may message or interrupt a child â€” enforced by child_id prefix
and parent-session matching.

Thread-safe. Never raises on misuse (tools return error strings instead).
"""

from __future__ import annotations

import os
import threading
import time
import uuid
from collections import deque
from typing import Any, Optional

from server.session.log import SessionManager, get_session, session_dir_for

_ACTIVATIONS: dict[str, dict[str, Any]] = {}
_ACTIVATIONS_LOCK = threading.RLock()

#: ContextVar naming the child whose turn is currently executing, so the
#: child-scoped ``report`` tool knows its parent chat (set by _run_turn).
import contextvars
current_child: contextvars.ContextVar[str] = contextvars.ContextVar(
    "current_child", default=""
)

_DESCRIPTOR = "subagent/descriptor"
_STATUS = "subagent/status"

_CONTINUABLE_PREFIX = "cont-"


def _sessions_dir(workspace: str) -> str:
    return session_dir_for(workspace)


def _activation(child_id: str) -> dict:
    with _ACTIVATIONS_LOCK:
        act = _ACTIVATIONS.get(child_id)
        if act is None:
            act = {
                "child_id": child_id,
                "status": "idle",
                "queue": deque(),
                "interrupt": False,
                "lock": threading.RLock(),
                "session_path": None,
                "name": "",
                "parent_chat_id": "",
            }
            _ACTIVATIONS[child_id] = act
        return act


def _new_child_id(chat_id: str, name: str) -> str:
    return f"{_CONTINUABLE_PREFIX}{chat_id}-{name}-{uuid.uuid4().hex[:8]}"


def _find_child_session(child_id: str, workspace: str) -> Optional[str]:
    """Locate a child session file by its id (header.chat_id == child_id)."""
    sdir = _sessions_dir(workspace)
    if not os.path.isdir(sdir):
        return None
    for fn in sorted(os.listdir(sdir), reverse=True):
        if not fn.endswith(".jsonl"):
            continue
        path = os.path.join(sdir, fn)
        try:
            mgr = SessionManager.open(path)
        except Exception:
            continue
        if mgr.get_header().chat_id == child_id:
            return path
    return None


def _descriptor_of(mgr: SessionManager) -> Optional[dict]:
    for e in mgr.get_entries():
        if getattr(e, "type", "") == "custom" and getattr(e, "custom_type", "") == _DESCRIPTOR:
            return e.data or {}
    return None


def _task_label(task: str) -> str:
    """Short human label for a spawn, derived from the task text (UI block title)."""
    text = " ".join((task or "").split())
    if not text:
        return ""
    cut = text[:42]
    if len(text) > 42:
        last_space = cut.rfind(" ")
        if last_space >= 18:
            cut = cut[:last_space]
        cut = cut.rstrip(" ,;:.") + "…"
    return cut


def _log_spawn(child_id: str, name: str, label: str) -> None:
    """Announce the spawn in the parent's live terminal log.

    The frontend's log parser turns this line into a clickable subagent block
    (child_id → drill-in session view) persisted in the parent's trace steps.
    """
    try:
        from sync.state_sync import safe_append_live_log
        safe_append_live_log(f"[SUBAGENT] child={child_id} name={name} label={label or name}")
    except Exception:
        pass


# â”€â”€ Build the child's system prompt from the subagent registry â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€


def _child_system_prompt(name: str) -> str:
    """Rebuild the child's system prompt from the registry spec (stable)."""
    try:
        from tools import _SUBAGENTS_REGISTRY, _load_subagents
        if not _SUBAGENTS_REGISTRY:
            _load_subagents()
        spec = _SUBAGENTS_REGISTRY.get(name) or {}
        return spec.get("system_prompt") or "You are a specialized subagent."
    except Exception:
        return "You are a specialized subagent."


def _child_permissions(name: str) -> list:
    """Build the child's FilesystemPermission list from its registry spec."""
    try:
        from tools import _SUBAGENTS_REGISTRY, _load_subagents, FilesystemPermission
        if not _SUBAGENTS_REGISTRY:
            _load_subagents()
        spec = _SUBAGENTS_REGISTRY.get(name) or {}
        out = []
        for p in spec.get("permissions", []):
            if isinstance(p, dict):
                out.append(FilesystemPermission(
                    operations=p.get("operations", []),
                    paths=p.get("paths", []),
                    mode=p.get("mode", "allow"),
                ))
            elif isinstance(p, FilesystemPermission):
                out.append(p)
        return out
    except Exception:
        return []


# â”€â”€ The turn loop â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€


def _is_native_slice(second: list) -> bool:
    """True when a backend result's 2nd element is a transcript slice.

    Handles both legacy LC (AIMessage/ToolMessage — test compat) and native
    ModelMessage (ModelResponse/ModelRequest) slices. Used by the
    continuable turn loop to decide whether the backend already executed
    tools (native full turn) or returned raw tool-call dicts.
    """
    try:
        for m in (second or [])[:2]:
            cname = m.__class__.__name__ if hasattr(m, "__class__") else ""
            if cname in ("AIMessage", "ToolMessage", "ModelResponse", "ModelRequest"):
                return True
            try:
                from pydantic_ai.messages import ModelRequest as _MR, ModelResponse as _MS
                if isinstance(m, (_MR, _MS)):
                    return True
            except Exception:
                pass
            if isinstance(m, dict) and ("tool" in m or "name" in m):
                return False
        # Fallback isinstance check across all items
        try:
            from pydantic_ai.messages import ModelRequest as _MR2, ModelResponse as _MS2
            if any(isinstance(m, (_MR2, _MS2)) for m in (second or [])):
                return True
        except Exception:
            pass
        return False
    except Exception:
        return False


def _run_turn(
    act: dict,
    session: SessionManager,
    name: str,
    extra_messages: list[str],
    reasoning_effort: Optional[str] = None,
) -> str:
    """Run one turn (user message -> loop -> done) on a continuable child.

    Native path (A6b): history is ``List[ModelMessage]`` built via
    :func:`server.session.log.convert_to_model_messages` + ``build_initial_history``
    helpers and ``ModelRequest(SystemPromptPart/UserPromptPart)``.  The backend
    is called with ``history`` (ModelMessage).  A native slice
    (``ModelResponse``/``ModelRequest``) is detected via ``_is_native_slice``
    and extended directly; legacy LC ``AIMessage``/``ToolMessage`` slices are
    kept for test compat via ``isinstance`` + ``_messages_to_pai`` fallback.

    Appends every step to the child session log. Cooperative with
    ``act["interrupt"]`` (checked per iteration). Returns the final text.
    """
    # Native imports — keep at function scope to avoid import cycles
    from server.session.log import convert_to_model_messages, build_session_context_native
    from llm.model_history import build_initial_history
    from pydantic_ai.messages import (
        ModelRequest,
        ModelResponse,
        SystemPromptPart,
        UserPromptPart,
        TextPart,
        ThinkingPart,
        ToolCallPart,
        ToolReturnPart,
    )
    from tools.AddOnTool.subagentTools import _specialist_call_backend
    from tools import (execute_tool_detailed, auto_offload_result, _tool_cap,
                       active_permissions, _TASK_DEPTH,
                       get_native_tools, _SUBAGENTS_REGISTRY, _load_subagents)

    with act["lock"]:
        act["status"] = "running"
        act["interrupt"] = False
    act["session_path"] = session.get_session_file()

    # Mirror task()'s context: depth 1, the spec's permissions.
    _depth_token = _TASK_DEPTH.set(1)
    _perm_token = active_permissions.set(_child_permissions(name))
    _descriptor = _descriptor_of(session) or {}
    _parent_chat = _descriptor.get("parent_chat_id", "") or ""

    interrupted = False

    try:
        # Context = full child history (compaction checkpoints honored) — native
        # Primary: entries -> convert_to_model_messages -> List[ModelMessage];
        # build history via build_initial_history or append (A6b).
        entries = session.build_context_entries()
        if not entries:
            # Fresh child — build initial history via helper (system + no user yet;
            # user task already stored as session entry but entries empty on cold start edge)
            history: list[Any] = build_initial_history(_child_system_prompt(name), "")
            # If session already has user entries, convert them instead (cold-resume)
            _conv = convert_to_model_messages(entries)
            if _conv:
                history = _conv
                has_sys_tmp = any(
                    isinstance(m, ModelRequest) and any(isinstance(p, SystemPromptPart) for p in getattr(m, "parts", []) or [])
                    for m in history
                )
                if not has_sys_tmp:
                    history.insert(0, ModelRequest(parts=[SystemPromptPart(content=_child_system_prompt(name))]))
        else:
            history = convert_to_model_messages(entries)
            # Ensure system prompt present (native) — insert via ModelRequest(SystemPromptPart)
            has_sys = any(
                isinstance(m, ModelRequest) and any(isinstance(p, SystemPromptPart) for p in getattr(m, "parts", []) or [])
                for m in history
            )
            if not has_sys:
                # Prefer build_initial_history for system prefix when history empty-ish,
                # else direct ModelRequest insert preserves existing context.
                if not history:
                    history = build_initial_history(_child_system_prompt(name), "")
                else:
                    history.insert(0, ModelRequest(parts=[SystemPromptPart(content=_child_system_prompt(name))]))

        # Codex-style plan re-injection: the child's plan is part of its session
        # state; restore it on resume. Empty for first turns (the model writes
        # the plan there via update_plan).
        try:
            from tools import get_plan_for_context
            plan_block = get_plan_for_context(name, _parent_chat)
            if plan_block:
                history.append(ModelRequest(parts=[UserPromptPart(content=plan_block)]))
        except Exception as e:
            print(f"[SUBAGENT] Error restoring plan block: {e}")

        for text in extra_messages:
            history.append(ModelRequest(parts=[UserPromptPart(content=text)]))
            session.append_message("user", text, source="followup")

        # Vision: inject user-attached images stashed by the API layer — native
        try:
            from llm.deps import WorkspaceDeps
            _sub_deps = WorkspaceDeps.from_shared_state(role=name, chat_id=_parent_chat)
            from tools.vision_channel import drain_attachment_images_native
            if drain_attachment_images_native(history, ctx=_sub_deps):
                session.append_message("system", "[VISION] user attachment image(s) injected")
        except Exception:
            pass

        if not _SUBAGENTS_REGISTRY:
            _load_subagents()
        spec = _SUBAGENTS_REGISTRY.get(name) or {}
        allowed_tools = set(spec.get("tools", []))
        native_tools = [
            definition for definition in get_native_tools()
            if definition.get("function", {}).get("name") in allowed_tools
        ]
        try:
            from tools.mcp.mcp_bridge import mcp_tool_defs_for_scope
            native_tools.extend(mcp_tool_defs_for_scope(name))
        except Exception:
            pass

        content = ""
        i = 0
        while True:
            i += 1
            if act["interrupt"]:
                interrupted = True
                break
            try:
                res = _specialist_call_backend(
                    name,
                    list(history),
                    native_tools=native_tools or None,
                    reasoning_effort=None,  # static per-role mapping in _build_model_settings
                    where=f"{name} continuable iter {i}",
                )
            except Exception:
                break

            reasoning = getattr(res, "reasoning_content", None)
            # A5: native full turn — pydantic-ai executed the tools internally.
            # The 3rd element is the transcript slice (ModelMessage or LC);
            # extend history + session log. Keep legacy LC fallback for tests.
            if isinstance(res, tuple) and len(res) == 3:
                candidate = res[1]
                is_slice = _is_native_slice(candidate)
                # Also try direct ModelMessage isinstance as extra guard
                if not is_slice:
                    try:
                        from pydantic_ai.messages import ModelRequest as _MRx, ModelResponse as _MSx
                        if candidate and isinstance(candidate[0], (_MRx, _MSx)):
                            is_slice = True
                    except Exception:
                        pass
                if is_slice:
                    response_text, slice_msgs, reasoning = res
                    reasoning = reasoning or getattr(res, "reasoning_content", None)
                    # Detect native vs LC slice
                    is_native_slice = False
                    try:
                        is_native_slice = bool(slice_msgs and isinstance(slice_msgs[0], (ModelRequest, ModelResponse)))
                    except Exception:
                        is_native_slice = False
                    if is_native_slice:
                        history.extend(slice_msgs)
                        slice_calls: list[dict] = []
                        for m in slice_msgs:
                            if isinstance(m, ModelResponse):
                                for p in getattr(m, "parts", []) or []:
                                    if isinstance(p, ToolCallPart):
                                        args = p.args if isinstance(p.args, dict) else {}
                                        if isinstance(args, str):
                                            try:
                                                import json
                                                args = json.loads(args)
                                            except Exception:
                                                args = {}
                                        slice_calls.append({
                                            "id": p.tool_call_id or "",
                                            "name": p.tool_name or "",
                                            "args": args if isinstance(args, dict) else {},
                                        })
                    else:
                        # Legacy LC slice (test compat) — convert to native for history
                        try:
                            from llm.pai_transport import _messages_to_pai
                            _converted = _messages_to_pai(slice_msgs)  # type: ignore
                            history.extend(_converted)
                        except Exception:
                            try:
                                history.extend(slice_msgs)  # type: ignore
                            except Exception:
                                pass
                        slice_calls = [
                            {"id": tc.get("id", ""), "name": tc.get("name", ""), "args": tc.get("args") or {}}
                            for m in slice_msgs
                            if m.__class__.__name__ == "AIMessage"
                            for tc in (getattr(m, "tool_calls", None) or [])
                        ]
                    blocks = [{"type": "text", "text": str(response_text)}]
                    if reasoning:
                        blocks.insert(0, {"type": "thinking", "thinking": str(reasoning)})
                    session.append_assistant(
                        blocks,
                        stop_reason="toolUse" if slice_calls else "stop",
                    )
                    if not slice_calls:
                        content = str(response_text)
                        break
                    for tc in slice_calls:
                        session.append_tool_result(
                            "ok",
                            tool_call_id=tc.get("id", ""),
                            tool_name=tc.get("name", ""),
                            is_error=False,
                        )
                    continue
            if isinstance(res, tuple):
                response_text, native_tool_calls = res[0], res[1]
                reasoning = reasoning or (res[2] if len(res) > 2 else None)
            else:
                response_text = getattr(res, "content", str(res))
            response_text = str(response_text)

            # Native tool calls only. The provider API owns the tool protocol;
            # pseudo-tool text is not generated or parsed in durable sessions.
            tool_calls = []
            for call_index, tc in enumerate(native_tool_calls or []):
                tname = tc.get("tool") or tc.get("name", "")
                if tname:
                    tool_calls.append({
                        "tool": tname,
                        "args": tc.get("args") or {},
                        "id": tc.get("id") or f"cont-{i}-{call_index + 1}",
                    })

            blocks = [{"type": "text", "text": response_text}]
            if reasoning:
                blocks.insert(0, {"type": "thinking", "thinking": str(reasoning)})

            if not tool_calls:
                session.append_assistant(blocks, stop_reason="stop")
                # Native history append — ModelResponse with TextPart/ThinkingPart
                _parts: list[Any] = []
                if reasoning:
                    _parts.append(ThinkingPart(content=str(reasoning)))
                if response_text:
                    _parts.append(TextPart(content=response_text))
                if _parts:
                    history.append(ModelResponse(parts=_parts))
                elif response_text or reasoning:
                    # Ensure at least one part
                    history.append(ModelResponse(parts=[TextPart(content=response_text or "")]))
                content = response_text
                break

            session.append_assistant(blocks, stop_reason="toolUse")
            # Native history: ModelResponse with ToolCallPart
            _resp_parts: list[Any] = []
            if response_text:
                _resp_parts.append(TextPart(content=response_text))
            if reasoning:
                _resp_parts.append(ThinkingPart(content=str(reasoning)))
            for tc in tool_calls:
                _resp_parts.append(ToolCallPart(
                    tool_name=tc["tool"],
                    args=tc.get("args") or {},
                    tool_call_id=tc.get("id", ""),
                ))
            history.append(ModelResponse(parts=_resp_parts))

            results = []
            for tc in tool_calls:
                tname = tc.get("tool", "")
                targs = tc.get("args", {})
                if act["interrupt"]:
                    interrupted = True
                    break
                call_id = tc["id"]
                try:
                    execution = execute_tool_detailed(tname, targs)
                    r = auto_offload_result(execution.content, tname, max_chars=_tool_cap("subagent"))
                    session.append_tool_result(r, tool_call_id=call_id, tool_name=tname)
                except Exception as e:
                    r = f"Error: {e}"
                    session.append_tool_result(
                        str(e)[:300], tool_call_id=call_id, tool_name=tname, is_error=True,
                    )
                results.append(str(r))
            if interrupted:
                break
            for tc, result in zip(tool_calls, results):
                history.append(ModelRequest(parts=[ToolReturnPart(
                    tool_name=tc.get("tool", ""),
                    content=str(result),
                    tool_call_id=tc["id"],
                )]))

            # Vision: drain tool-queued inline images into ONE user message — native
            try:
                from llm.deps import WorkspaceDeps
                _sub_deps = WorkspaceDeps.from_shared_state(role=name, chat_id=_parent_chat)
                from tools.vision_channel import drain_into_messages_native
                if drain_into_messages_native(history, header="[Inline image(s) from tool result]", ctx=_sub_deps):
                    session.append_message("system", "[VISION] inline image(s) injected")
            except Exception:
                pass

        if interrupted:
            content = content or "INTERRUPTED"
        session.append_custom_entry(_STATUS, {"status": "idle", "at": int(time.time() * 1000)})
        return content
    finally:
        active_permissions.reset(_perm_token)
        _TASK_DEPTH.reset(_depth_token)
        with act["lock"]:
            act["status"] = "idle"
            act["interrupt"] = False


# ── Public API ───────────────────────────────────────────────────────────
def start_continuable(
    chat_id: str,
    workspace: str,
    name: str,
    task: str,
    label: Optional[str] = None,
    parent_session: Optional[str] = None,
    reasoning_effort: Optional[str] = None,
) -> dict:
    """Create a durable continuable child and run its first turn (blocking).

    Returns ``{"child_id", "result", "status"}``. The child persists; later
    :func:`send_message` turns resume it from its log.
    """
    child_id = _new_child_id(chat_id, name)
    session = get_session(
        child_id, workspace,
        parent_session=parent_session,
        delegation_depth=1,
        chat_name=label or name,
    )
    spawn_label = label or _task_label(task) or name
    session.append_custom_entry(_DESCRIPTOR, {
        "name": name,
        "label": spawn_label,
        "parent_session": parent_session,
        "parent_chat_id": chat_id,
        "created": int(time.time() * 1000),
    })
    session.append_message("user", task, source="user")
    _log_spawn(child_id, name, spawn_label)

    act = _activation(child_id)
    act["name"] = name
    act["parent_chat_id"] = chat_id
    act["session_path"] = session.get_session_file()

    token = current_child.set(child_id)
    try:
        result = _run_turn(act, session, name, extra_messages=[], reasoning_effort=reasoning_effort)
    finally:
        current_child.reset(token)

    session.append_custom_entry(_STATUS, {"status": "idle", "at": int(time.time() * 1000)})
    return {"child_id": child_id, "result": result, "status": "idle"}


def send_message(
    child_id: str, message: str, workspace: str, timeout: float = 1800.0
) -> dict:
    """Deliver a follow-up turn to a continuable child (FIFO).

    A running child queues the message; an idle or cold child resumes and runs
    it now (blocking up to ``timeout``). Returns {"result", "queued", "status"}.
    """
    act = _activation(child_id)
    with act["lock"]:
        if act["status"] == "running":
            act["queue"].append(message)
            return {"result": "", "queued": True, "status": "running"}

        act["queue"].append(message)
        path = act["session_path"] or _find_child_session(child_id, workspace)
        if path is None:
            act["queue"].clear()
            return {"result": f"Error: unknown continuable child '{child_id}'", "queued": False, "status": "error"}

    session = SessionManager.open(path)
    desc = _descriptor_of(session) or {}
    name = desc.get("name") or child_id.split("-")[2] if len(child_id.split("-")) > 2 else "agent"

    with act["lock"]:
        pending = list(act["queue"])
        act["queue"].clear()
    if not pending:
        return {"result": "", "queued": False, "status": "idle"}

    token = current_child.set(child_id)
    try:
        result = _run_turn(act, session, name, extra_messages=pending)
    finally:
        current_child.reset(token)
    return {"result": result, "queued": False, "status": "idle"}


def interrupt(child_id: str) -> bool:
    """Cooperatively interrupt a running child (keeps its queued messages)."""
    with _ACTIVATIONS_LOCK:
        act = _ACTIVATIONS.get(child_id)
    if act is None:
        return False
    with act["lock"]:
        act["interrupt"] = True
    return True


def list_agents(workspace: str, chat_id: Optional[str] = None) -> list[dict]:
    """Durable enumeration of children (no resume needed).

    Reads session headers + descriptor/status entries from disk, merged with
    live activation state. Covers both child kinds: ``cont-`` continuable
    children (descriptor + status) and ``sub-`` task() spawn logs (name parsed
    from the id, status always idle — they have no activation tracking).
    """
    sdir = _sessions_dir(workspace)
    out: list[dict] = []
    if not os.path.isdir(sdir):
        return out
    for fn in sorted(os.listdir(sdir), reverse=True):
        if not fn.endswith(".jsonl"):
            continue
        path = os.path.join(sdir, fn)
        try:
            mgr = SessionManager.open(path)
        except Exception:
            continue
        hdr = mgr.get_header()
        cid = hdr.chat_id or ""
        is_cont = cid.startswith(_CONTINUABLE_PREFIX)
        is_sub = cid.startswith("sub-")
        if not is_cont and not is_sub:
            continue
        if chat_id:
            if is_cont and not cid.startswith(f"{_CONTINUABLE_PREFIX}{chat_id}-"):
                continue
            if is_sub and not cid.startswith(f"sub-{chat_id}-"):
                continue
        desc = _descriptor_of(mgr) or {}
        status = "idle"
        for e in reversed(mgr.get_entries()):
            if getattr(e, "type", "") == "custom" and getattr(e, "custom_type", "") == _STATUS:
                status = (e.data or {}).get("status", "idle")
                break
        live = _ACTIVATIONS.get(cid)
        live_status = live["status"] if live else None
        if live_status == "running":
            status = "running"
        if is_cont:
            name = desc.get("name") or cid.split("-")[2] if len(cid.split("-")) > 2 else ""
            label = desc.get("label") or desc.get("name") or cid
            created = desc.get("created") or 0
        else:
            parts = cid.split("-")
            name = desc.get("name") or (parts[-2] if len(parts) >= 3 else cid)
            label = desc.get("label") or desc.get("name") or name or cid
            try:
                created = desc.get("created") or int(os.path.getmtime(path) * 1000)
            except Exception:
                created = 0
        out.append({
            "child_id": cid,
            "name": name,
            "label": label,
            "status": status,
            "created": created,
            "parent_session": hdr.parent_session,
        })
    out.sort(key=lambda a: a["created"], reverse=True)
    return out


def read_child_session(workspace: str, child_id: str) -> Optional[dict]:
    """Read a continuable child's session for the UI drill-in view.

    Returns ``{"info": {...}, "entries": [...]}`` or None when the child is
    unknown. Entries are a display projection: user / assistant text and tool
    results (tool outputs capped, thinking excluded, toolCall blocks folded
    into their toolResult rows via tool_call_id pairing).
    """
    path = _find_child_session(child_id, workspace)
    if path is None:
        return None
    try:
        mgr = SessionManager.open(path)
    except Exception:
        return None
    desc = _descriptor_of(mgr) or {}
    status = "idle"
    for e in reversed(mgr.get_entries()):
        if getattr(e, "type", "") == "custom" and getattr(e, "custom_type", "") == _STATUS:
            status = (e.data or {}).get("status", "idle")
            break
    with _ACTIVATIONS_LOCK:
        live = _ACTIVATIONS.get(child_id)
    if live and live.get("status") == "running":
        status = "running"

    # Descriptor-less spawns (task()'s sub- logs): parse the agent name from
    # the id tail ("sub-<chatId>-<Name>-<hex>").
    parsed_name = ""
    parts = child_id.split("-")
    if len(parts) >= 3:
        parsed_name = parts[-2]

    def _text_of(entry) -> str:
        content = entry.content
        if isinstance(content, str):
            return content
        parts = []
        for b in content:
            if getattr(b, "type", "") == "text":
                parts.append(b.text)
        return "\n".join(parts)

    entries: list[dict] = []
    pending_calls: dict[str, str] = {}
    for e in mgr.get_entries():
        if getattr(e, "type", "") != "message":
            continue
        if e.role == "user":
            text = _text_of(e)
            if text.strip():
                entries.append({"kind": "user", "content": text, "ts": e.timestamp})
        elif e.role == "assistant":
            content = e.content
            if isinstance(content, list):
                for b in content:
                    if getattr(b, "type", "") == "toolCall" and getattr(b, "id", ""):
                        pending_calls[b.id] = b.name
            text = _text_of(e)
            if text.strip():
                entries.append({"kind": "assistant", "content": text, "ts": e.timestamp})
        elif e.role == "toolResult":
            tool_name = e.tool_name or pending_calls.get(e.tool_call_id or "", "tool")
            entries.append({
                "kind": "tool",
                "toolName": tool_name,
                "content": _text_of(e)[:4000],
                "isError": bool(e.is_error),
                "ts": e.timestamp,
            })

    return {
        "info": {
            "child_id": child_id,
            "name": desc.get("name") or parsed_name or child_id,
            "label": desc.get("label") or desc.get("name") or parsed_name or child_id,
            "status": status,
            "created": desc.get("created") or mgr.get_header().timestamp,
            "parent_chat_id": desc.get("parent_chat_id") or "",
            "session_file": path,
        },
        "entries": entries,
    }


def report(message: str, quiet: bool = False) -> str:
    """Child â†’ parent report. Wakeup (default) queues a follow-up to the
    parent chat; quiet only records the message in the child's log."""
    child_id = current_child.get()
    if not child_id:
        return "Error: report can only be called from a continuable subagent turn."
    with _ACTIVATIONS_LOCK:
        act = _ACTIVATIONS.get(child_id)
    if act is None:
        return "Error: no parent context for this child."
    parent_chat = act.get("parent_chat_id") or ""
    if not parent_chat:
        return "Error: child has no parent chat."
    if not quiet:
        try:
            from inbox import follow_up
            follow_up(parent_chat, f"[SUBAGENT REPORT]\n{message}")
            return "[OK] Report delivered to parent."
        except Exception as e:
            return f"Error delivering report: {e}"
    return "[OK] Report recorded (quiet)."


def reset_manager() -> None:
    """Test helper."""
    with _ACTIVATIONS_LOCK:
        _ACTIVATIONS.clear()

