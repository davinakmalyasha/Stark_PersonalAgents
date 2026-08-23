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

from session_log import SessionManager, get_session, session_dir_for

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


# â”€â”€ Build the child's system prompt from the subagent registry â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€


def _child_system_prompt(name: str) -> str:
    """Rebuild the child's system prompt from the registry spec (stable)."""
    try:
        from tools import _SUBAGENTS_REGISTRY, _load_subagents
        if not _SUBAGENTS_REGISTRY:
            _load_subagents()
        spec = _SUBAGENTS_REGISTRY.get(name) or {}
        sys_prompt = spec.get("system_prompt") or "You are a specialized subagent."
        tool_names = spec.get("tools", [])
        if tool_names:
            try:
                from tools import ensure_tool_registry
                reg = ensure_tool_registry()
                reg.restrict_scope(name, allow=list(tool_names))
            except Exception:
                pass
        return sys_prompt
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


def _run_turn(act: dict, session: SessionManager, name: str, extra_messages: list[str]) -> str:
    """Run one turn (user message -> loop -> done) on a continuable child.

    Appends every step to the child session log. Cooperative with
    ``act["interrupt"]`` (checked per iteration). Returns the final text.
    """
    from langchain_core.messages import AIMessage, HumanMessage, SystemMessage, ToolMessage
    from llm import invoke_messages_with_fallback_stream
    from tools import (execute_tool_detailed, auto_offload_result, _tool_cap,
                       active_permissions, _TASK_DEPTH, active_tool_scope,
                       get_native_tools, _SUBAGENTS_REGISTRY, _load_subagents,
                       _current_chat)
    from session_log import convert_to_llm

    with act["lock"]:
        act["status"] = "running"
        act["interrupt"] = False
    act["session_path"] = session.get_session_file()

    # Mirror task()'s context: depth 1, the spec's permissions, tool scope
    _depth_token = _TASK_DEPTH.set(1)
    _perm_token = active_permissions.set(_child_permissions(name))
    _scope_token = active_tool_scope.set(name)
    _descriptor = _descriptor_of(session) or {}
    _parent_chat = _descriptor.get("parent_chat_id", "") or ""
    _chat_token = _current_chat.set(_parent_chat)

    interrupted = False

    try:
        # Context = full child history (compaction checkpoints honored)
        entries = session.build_context_entries()
        messages = convert_to_llm(entries, native_tool_calls=True)
        has_sys = any(type(m).__name__ == "SystemMessage" for m in messages)
        if not has_sys:
            messages.insert(0, SystemMessage(content=_child_system_prompt(name)))

        # Codex-style plan re-injection: the child's plan is part of its session
        # state; restore it on resume. Empty for first turns (the model writes
        # the plan there via update_plan).
        try:
            from tools import get_plan_for_context
            plan_block = get_plan_for_context(name, _parent_chat)
            if plan_block:
                messages.append(HumanMessage(content=plan_block))
        except Exception as e:
            print(f"[SUBAGENT] Error restoring plan block: {e}")

        for text in extra_messages:
            messages.append(HumanMessage(content=text))
            session.append_message("user", text, source="followup")

        # Vision: inject user-attached images stashed by the API layer.
        try:
            from tools.vision_channel import drain_attachment_images
            if drain_attachment_images(messages):
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
                res = invoke_messages_with_fallback_stream(
                    role=name,
                    messages=list(messages),
                    temp=0.2,
                    tools=native_tools or None,
                    reasoning_effort=os.environ.get("REASONING_SUBAGENT_ROUTINE", "disabled"),
                    where=f"{name} continuable iter {i}",
                )
            except Exception:
                break

            reasoning = getattr(res, "reasoning_content", None)
            native_tool_calls = None
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
                messages.append(AIMessage(content=response_text))
                content = response_text
                break

            session.append_assistant(blocks, stop_reason="toolUse")
            messages.append(AIMessage(content=response_text, tool_calls=[
                {"name": tc["tool"], "args": tc.get("args") or {}, "id": tc.get("id", "")}
                for tc in tool_calls
            ]))

            results = []
            for tc in tool_calls:
                tname = tc.get("tool", "")
                targs = tc.get("args", {})
                if act["interrupt"]:
                    interrupted = True
                    break
                call_id = tc["id"]
                try:
                    execution = execute_tool_detailed(tname, targs, scope=name)
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
                messages.append(ToolMessage(
                    content=str(result),
                    tool_call_id=tc["id"],
                    name=tc.get("tool", ""),
                ))

            # Vision: drain tool-queued inline images into ONE user message.
            try:
                from tools.vision_channel import drain_into_messages
                if drain_into_messages(messages, header="[Inline image(s) from tool result]"):
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
        active_tool_scope.reset(_scope_token)
        _current_chat.reset(_chat_token)
        with act["lock"]:
            act["status"] = "idle"
            act["interrupt"] = False


# â”€â”€ Public API â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€


def start_continuable(
    chat_id: str,
    workspace: str,
    name: str,
    task: str,
    label: Optional[str] = None,
    parent_session: Optional[str] = None,
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
    session.append_custom_entry(_DESCRIPTOR, {
        "name": name,
        "label": label or name,
        "parent_session": parent_session,
        "parent_chat_id": chat_id,
        "created": int(time.time() * 1000),
    })
    session.append_message("user", task, source="user")

    act = _activation(child_id)
    act["name"] = name
    act["parent_chat_id"] = chat_id
    act["session_path"] = session.get_session_file()

    token = current_child.set(child_id)
    try:
        result = _run_turn(act, session, name, extra_messages=[])
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
    """Durable enumeration of continuable children (no resume needed).

    Reads session headers + descriptor/status entries from disk, merged with
    live activation state.
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
        if not cid.startswith(_CONTINUABLE_PREFIX):
            continue
        if chat_id and not cid.startswith(f"{_CONTINUABLE_PREFIX}{chat_id}-"):
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
        out.append({
            "child_id": cid,
            "name": desc.get("name") or cid.split("-")[2] if len(cid.split("-")) > 2 else "",
            "label": desc.get("label") or desc.get("name") or cid,
            "status": status,
            "created": desc.get("created") or 0,
            "parent_session": hdr.parent_session,
        })
    out.sort(key=lambda a: a["created"], reverse=True)
    return out


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
