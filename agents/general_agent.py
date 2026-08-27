"""
general_agent.py --- General subagent: non-coding life & work assistant.

Sits at the same tier as Dev (below the Orchestrator) but owns everything
OUTSIDE software development: research, documents, planning, organization,
web lookups, and personal admin tasks.

Two entry points share ONE core loop (:func:`general_loop`):

- ``task("General", ...)`` delegation -> :func:`general_node` (blocking,
  returns a compact handoff string via the registry's ``_handoff``).
- API General-mode chat -> :func:`run_general_direct` (generator yielding
  serialized state snapshots + stream-bus deltas).

Dual LLM backend via ``GENERAL_MODEL_BACKEND``:

- ``deepseek`` (default): invoke_messages_with_fallback_stream on
  ``deepseek:v4-flash`` with native function calling.
- ``free``: routes every turn through the multi-provider FreeAgent client
  (OpenRouter/Groq/Gemini/Mistral/NVIDIA/Cloudflare/...) — zero cost, same
  OpenAI-native tool protocol.
"""

from __future__ import annotations

import json
import os
import re
import uuid

from langchain_core.messages import AIMessage, HumanMessage, SystemMessage, ToolMessage

from sync.state_sync import safe_update_state, safe_append_live_log
from agents.it_department_nodes_base import ITState
from tools import (
    execute_tool_detailed,
    get_native_tools,
    get_native_tool_names,
    get_registry,
)
from llm import invoke_messages_with_fallback_stream
from llm.loop_detector import LoopGuard
from memory.context_compaction import (
    compact_history,
    get_compaction_policy,
)
from agents.general_prompt import _GENERAL_SYSTEM_TEMPLATE

_GENERAL_MODEL = "deepseek:v4-flash"
_AGENT_NAME = "General"

_HARNESS_ROOT = os.environ.get("HARNESS_ROOT") or os.path.dirname(
    os.path.dirname(os.path.abspath(__file__))
)


def _log(msg: str) -> None:
    """Console + shared-state live terminal."""
    try:
        print(msg, flush=True)
    except Exception:
        pass
    safe_append_live_log(msg)


def _profile_path() -> str:
    root = os.environ.get("HARNESS_ROOT") or _HARNESS_ROOT
    return os.path.join(root, ".deep_agents", "general_profile.json")


# ── Context builders ─────────────────────────────────────────────────────────


def _load_general_profile() -> str:
    """Render the General-mode user profile (.deep_agents/general_profile.json)
    into a compact context block. Empty string when absent."""
    try:
        with open(_profile_path(), "r", encoding="utf-8") as f:
            data = json.load(f)
    except Exception:
        return ""

    lines: list[str] = []
    info = data.get("user_info") or {}
    if isinstance(info, dict):
        for key in ("name", "birthday", "education"):
            val = str(info.get(key) or "").strip()
            if val:
                lines.append(f"- {key}: {val}")

    instructions = str(data.get("instructions") or "").strip()
    memories = [str(m).strip() for m in (data.get("memory") or []) if str(m).strip()]

    if not lines and not instructions and not memories:
        return ""

    out = ["## User Profile"]
    if lines:
        out.append("### User Info")
        out.extend(lines)
    if instructions:
        out.append("### Standing Instructions")
        out.append(instructions)
    if memories:
        out.append("### Memory")
        out.extend(f"- {m}" for m in memories[:20])
    return "\n".join(out)


def _build_zone2(task: str, chat_id: str) -> str:
    """Volatile context: profile, chat history, then the request (cache break)."""
    parts: list[str] = []

    profile = _load_general_profile()
    if profile:
        parts.append(profile)

    if chat_id:
        try:
            from memory.chat_context import build_chat_context

            history = build_chat_context(
                "_general", chat_id, max_messages=30, max_chars_per_msg=3000, exclude_traces=True
            )
            if history:
                parts.append(f"## Recent Chat History\n{history}")
        except Exception as e:
            _log(f"[{_AGENT_NAME.upper()}] Chat history load skipped: {e}")

    parts.append(f"## Request\n{task}")
    return "\n\n".join(parts)


def _clean_text(text: str) -> str:
    if not text:
        return ""
    cleaned = re.sub(r"<thinking>.*?</thinking>", "", text, flags=re.DOTALL)
    return cleaned.strip()


# ── LLM backends ─────────────────────────────────────────────────────────────


def _lc_to_native(messages: list) -> list[dict]:
    """LangChain transcript -> OpenAI-native dicts (free-provider transport).
    Image content-block lists pass through untouched."""
    out: list[dict] = []
    for m in messages:
        kind = type(m).__name__
        content = getattr(m, "content", "")
        if kind == "SystemMessage":
            out.append({"role": "system", "content": content})
        elif kind == "HumanMessage":
            out.append({"role": "user", "content": content})
        elif kind == "AIMessage":
            entry: dict = {"role": "assistant", "content": content or ""}
            tcs = getattr(m, "tool_calls", None) or []
            if tcs:
                entry["tool_calls"] = [
                    {
                        "id": tc.get("id", ""),
                        "type": "function",
                        "function": {
                            "name": tc.get("name") or tc.get("tool", ""),
                            "arguments": json.dumps(
                                tc.get("args") or {}, separators=(",", ":")
                            ),
                        },
                    }
                    for tc in tcs
                    if isinstance(tc, dict)
                ]
            out.append(entry)
        elif kind == "ToolMessage":
            out.append(
                {
                    "role": "tool",
                    "tool_call_id": getattr(m, "tool_call_id", "") or "",
                    "name": getattr(m, "name", "") or "",
                    "content": str(content),
                }
            )
    return out


def _normalize_calls(raw_calls) -> list[dict]:
    calls: list[dict] = []
    for index, tc in enumerate(raw_calls or []):
        if not isinstance(tc, dict):
            continue
        name = tc.get("tool") or tc.get("name", "")
        args = tc.get("args") or {}
        if isinstance(args, str):
            try:
                args = json.loads(args or "{}")
            except Exception:
                continue
        if name and isinstance(args, dict):
            calls.append(
                {
                    "tool": name,
                    "args": args,
                    "id": tc.get("id") or f"gen-{index + 1}",
                }
            )
    return calls


def _call_backend(messages: list, native_tools: list[dict], where_tag: str) -> tuple[str, list[dict]]:
    """One LLM turn on the configured backend.

    Returns ``(text, tool_calls)``. Raises on total provider failure.

    Backend resolution: GENERAL_MODEL_BACKEND env wins; otherwise the active
    provider mode's workspace-tier plan decides (see llm/provider_modes.py).
    """
    plan: dict = {}
    backend = os.environ.get("GENERAL_MODEL_BACKEND", "").strip().lower()
    if not backend:
        try:
            from llm.provider_modes import get_role_plan

            plan = get_role_plan(_AGENT_NAME)
            backend = str(plan.get("backend") or "paid")
        except Exception:
            plan = {}
            backend = "deepseek"

    if backend == "free":
        from llm.free_providers import free_native_turn

        reply, calls, _reasoning = free_native_turn(
            _AGENT_NAME,
            messages,
            tools=native_tools,
            capability="text",
            prefer_reasoning=bool(plan.get("think_bias", True)),
        )
        return reply, calls

    # Paid path (direct DeepSeek / OpenCode Go / Command Code gateway).
    effort_override = os.environ.get("REASONING_SUBAGENT_ROUTINE", "")
    if not effort_override:
        effort_override = str(plan.get("iter_effort") or "disabled")

    res = invoke_messages_with_fallback_stream(
        role=_AGENT_NAME,
        messages=list(messages),
        temp=0.2,
        tools=native_tools,
        model=_GENERAL_MODEL,
        reasoning_effort=effort_override,
        where=where_tag,
    )
    native_tool_calls: list = []
    if isinstance(res, tuple):
        if len(res) >= 3:
            response_text, native_tool_calls = res[0], res[1]
        elif len(res) == 2:
            response_text, native_tool_calls = res
        else:
            response_text = res[0] if res else ""
    elif hasattr(res, "content"):
        from llm import _extract_text

        response_text = _extract_text(res.content)
    else:
        response_text = str(res)

    return str(response_text or ""), _normalize_calls(native_tool_calls)


# ── Session log (P1, best-effort) ────────────────────────────────────────────


def _open_session(chat_id: str, task: str):
    try:
        from uuid import uuid4

        from session_log import find_recent_session, get_session
        from sync.state_sync import shared_state

        ws = shared_state.get("project_path") or _HARNESS_ROOT
        parent_path = find_recent_session(chat_id, ws) if chat_id else None
        sess = get_session(
            f"sub-{chat_id or 'general'}-{_AGENT_NAME}-{uuid4().hex[:6]}",
            ws,
            parent_session=parent_path,
        )
        sess.append_message("user", task, source="user")
        return sess
    except Exception:
        return None


# ── Core loop ────────────────────────────────────────────────────────────────


def _general_events(task: str, chat_id: str = "", images: list | None = None):
    """Core agent loop as an event generator.

    Yields ``("status", line)`` events as the loop progresses and finishes
    with ``("done", final_report)``. Both entry points consume this generator:
    the node wrapper drains it silently, direct mode forwards each event as a
    fresh state snapshot so the UI sees progress live.
    """
    valid_tools = get_native_tool_names("general")
    native_tools = get_native_tools("general")
    valid_tool_set = set(valid_tools)
    try:
        get_registry().restrict_scope(_AGENT_NAME, allow=list(valid_tools))
    except Exception:
        pass

    messages = [
        SystemMessage(content=_GENERAL_SYSTEM_TEMPLATE),
        HumanMessage(content=_build_zone2(task, chat_id)),
    ]

    # User-attached images: explicit paths first, then the API stash channel.
    if images:
        try:
            from tools.vision_channel import queue_image_file

            for p in images:
                queue_image_file(p, note="user attachment")
        except Exception:
            pass
    try:
        from tools.vision_channel import drain_attachment_images

        if drain_attachment_images(messages):
            _log(f"[VISION] {_AGENT_NAME}: user attachment image(s) injected")
    except Exception:
        pass

    sess = _open_session(chat_id, task)

    compaction_policy = get_compaction_policy("general")
    tool_call_log: list[dict] = []
    existing_ids = {m.id for m in messages if getattr(m, "id", None)}
    iteration = 0
    final_text = ""

    yield "status", f"[{_AGENT_NAME.upper()}] Starting general task loop"

    while True:
        iteration += 1
        yield "status", f"[{_AGENT_NAME.upper()}] Iteration {iteration}"

        # Inbox: a user abort lands at the next step boundary.
        if chat_id:
            try:
                from inbox import consume_abort

                if consume_abort(chat_id):
                    yield "status", f"[{_AGENT_NAME.upper()}] Cancelled by user request."
                    final_text = final_text or "Cancelled by user."
                    break
            except Exception:
                pass

        # Append-only compaction below threshold; single checkpoint at it.
        if iteration > 1:
            try:
                messages, compaction_info = compact_history(
                    messages,
                    tool_call_log=tool_call_log,
                    policy=compaction_policy,
                    chat_id=chat_id,
                )
                if compaction_info.get("checkpoint"):
                    _log(
                        f"[{_AGENT_NAME.upper()}] {compaction_info.get('trigger')} checkpoint "
                        f"({compaction_info['before']:,} -> {compaction_info['after']:,} est. tokens)"
                    )
            except Exception as e:
                _log(f"[{_AGENT_NAME.upper()}] Compaction error (non-fatal): {e}")

        last_entry = tool_call_log[-1] if tool_call_log else {}
        last_tool = last_entry.get("tool", "")
        where_tag = (
            f"{_AGENT_NAME} iter {iteration}: {last_tool}"
            if last_tool
            else f"{_AGENT_NAME} iter {iteration}: start"
        )

        try:
            response_text, tool_calls = _call_backend(messages, native_tools, where_tag)
        except Exception as e:
            _log(f"[{_AGENT_NAME.upper()}] LLM error after all fallbacks: {e}")
            raise

        if not response_text.strip() and not tool_calls:
            messages.append(HumanMessage(
                content=(
                    "[NUDGE] You responded with nothing. Continue the request with a tool, "
                    "or if finished, summarize what you accomplished.",
                ),
                id=f"gen-nudge-{iteration}-{uuid.uuid4().hex[:8]}",
            ))
            continue

        if not tool_calls:
            final_text = _clean_text(response_text)
            messages.append(AIMessage(content=response_text))
            if sess is not None:
                try:
                    sess.append_assistant(final_text, stop_reason="stop")
                except Exception:
                    pass
            break

        ai_msg = AIMessage(
            content=response_text,
            id=f"gen-ai-{iteration}-{uuid.uuid4().hex[:8]}",
            tool_calls=[
                {"name": tc["tool"], "args": tc.get("args") or {}, "id": tc.get("id", "")}
                for tc in tool_calls
            ],
        )
        messages.append(ai_msg)
        if sess is not None:
            try:
                sess.append_assistant(response_text, stop_reason="toolUse")
            except Exception:
                pass

        for tc in tool_calls:
            tname = tc["tool"]
            targs = tc.get("args") or {}
            call_id = tc.get("id") or f"gen-call-{iteration}-{len(tool_call_log)}"

            guard = LoopGuard.check_pre_execute(tool_call_log, tname, targs)

            if tname not in valid_tool_set:
                result_str = (
                    f"[BLOCKED] Tool '{tname}' is not available to {_AGENT_NAME}. "
                    f"Available: {', '.join(sorted(valid_tool_set))}"
                )
            else:
                try:
                    execution = execute_tool_detailed(tname, targs, scope=_AGENT_NAME)
                    result_str = str(execution.content)
                except Exception as e:
                    result_str = f"ERROR: {str(e)[:300]}"

            # Offload oversized results to VFS; keep the transcript compact.
            try:
                from tools import _tool_cap, auto_offload_result

                result_str = auto_offload_result(result_str, tname, max_chars=_tool_cap("subagent"))
            except Exception:
                if len(result_str) > 4000:
                    result_str = result_str[:2000] + "\n... (truncated middle) ...\n" + result_str[-2000:]

            if guard and guard[0] == "WARNING":
                result_str += "\n" + guard[1]

            yield "status", f"[{_AGENT_NAME.upper()}] {tname}: {result_str[:120]}"
            tool_call_log.append({
                "tool": tname,
                "args": targs,
                "iteration": iteration,
                "status": "error" if result_str.startswith(("ERROR:", "[BLOCKED]")) else "ok",
            })
            messages.append(ToolMessage(
                content=result_str,
                tool_call_id=call_id,
                name=tname,
            ))
            if sess is not None:
                try:
                    sess.append_tool_result(
                        result_str,
                        tool_call_id=call_id,
                        tool_name=tname,
                        is_error=result_str.startswith(("ERROR:", "[BLOCKED]")),
                    )
                except Exception:
                    pass

        # Inline images queued by tools land as ONE synthetic user message.
        try:
            from tools.vision_channel import drain_into_messages

            drain_into_messages(messages, header="[Inline image(s) from tool result]")
        except Exception:
            pass

        # Steering lands at the next step boundary.
        if chat_id:
            try:
                from inbox import drain_steering, render_delivery

                steer = drain_steering(chat_id)
                if steer:
                    messages.append(HumanMessage(
                        content=render_delivery("STEERING", steer),
                        id=f"gen-steer-{iteration}-{uuid.uuid4().hex[:8]}",
                    ))
            except Exception:
                pass

    new_msgs = [m for m in messages if getattr(m, "id", None) not in existing_ids]
    yield "messages", new_msgs
    yield "done", final_text or "No final response produced."


def general_loop(
    task: str,
    chat_id: str = "",
    images: list | None = None,
    emit=None,
) -> str:
    """Run the General agent loop to completion. Returns the final report text.

    ``emit(status_line)`` optionally receives live progress lines.
    """
    new_msgs: list = []
    final_text = ""
    for kind, payload in _general_events(task, chat_id=chat_id, images=images):
        if kind == "status":
            _log(payload)
            if callable(emit):
                try:
                    emit(payload)
                except Exception:
                    pass
        elif kind == "messages":
            new_msgs = payload
        elif kind == "done":
            final_text = payload
    general_loop.last_messages = new_msgs
    return final_text


def shared_state_chat_id() -> str:
    try:
        from sync.state_sync import shared_state

        return str(shared_state.get("chat_id") or "")
    except Exception:
        return ""


def run_general_direct(prompt: str, chat_id: str = "", images: list | None = None):
    """Generator entry point for the API server's General mode.

    Consumes the core event generator and yields a serialized state snapshot
    per event (same contract as the old run_freeagent_direct), so the frontend
    sees terminal-log progress live.
    """
    from sync.state_sync import safe_serialize_state

    safe_update_state({
        "live_terminal_log": f"[{_AGENT_NAME}] Processing...\n",
        "outputs": {},
    })
    yield json.loads(safe_serialize_state())

    emit_chat = chat_id or shared_state_chat_id()
    last_status = ""

    try:
        for kind, payload in _general_events(prompt, chat_id=emit_chat, images=images):
            if kind == "status" and payload != last_status:
                last_status = payload
                safe_update_state({"live_terminal_log": payload + "\n"})
                yield json.loads(safe_serialize_state())
            elif kind == "done":
                safe_update_state({
                    "live_terminal_log": f"[{_AGENT_NAME}] Done\n",
                    "outputs": {"agent_report": (payload or "").strip()},
                })
                yield json.loads(safe_serialize_state())
            # "messages" events are graph-only; skip them here.
    except Exception as e:
        import traceback

        safe_update_state({
            "error": str(e),
            "live_terminal_log": f"[{_AGENT_NAME} Error] {e}\n{traceback.format_exc()}",
        })
        yield json.loads(safe_serialize_state())


# ── Entry points ─────────────────────────────────────────────────────────────


def general_node(s: ITState) -> dict:
    """Graph/registry-facing node. Called from task('General', ...) via the
    special-case branch in subagentTools.task()."""
    client_req = s.get("client_request", "") if hasattr(s, "get") else ""
    chat_id = s.get("chat_id", "") if hasattr(s, "get") else ""
    project_path = s.get("project_path", "") if hasattr(s, "get") else ""
    images = s.get("images") if hasattr(s, "get") else None
    safe_update_state({"thoughts": {"general": "Analyzing and executing..."}})
    try:
        report = general_loop(client_req, chat_id=chat_id, images=list(images) if images else None)
        safe_update_state({
            "thoughts": {"general": "Task complete."},
            "outputs": {"agent_report": report},
        })
        return {
            "messages": list(getattr(general_loop, "last_messages", []) or []),
            "agent_report": report,
            "code": "",
            "test_report": "",
            "project_path": project_path,
            "code_updated": False,
            "tech_spec_updated": False,
            "next_agent": "finish",
        }
    except Exception as e:
        import traceback

        tb = traceback.format_exc()
        _log(f"[{_AGENT_NAME.upper()} CRASH] {e}\n{tb}")
        safe_append_live_log(f"\n[{_AGENT_NAME.upper()} CRASH] {e}")
        return {
            "messages": [],
            "agent_report": f"ERROR: General agent crashed: {e}",
            "code": f"ERROR: {e}",
            "test_report": "",
            "project_path": project_path,
            "code_updated": False,
            "tech_spec_updated": False,
            "next_agent": "finish",
            "error": str(e),
        }
