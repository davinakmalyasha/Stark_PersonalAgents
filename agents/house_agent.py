"""
house_agent.py --- House subagent: home control specialist.

Sits at the same tier as Dev and General (below the Orchestrator) and owns
the smart home: reads and actuates the HouseState through the house_* tools
(lights, thermostats, locks, plugs, blinds, cameras, sensors, scenes).

Entry points share ONE core loop (:func:`_house_events`):

- ``task("House", ...)`` delegation -> :func:`house_node` (blocking,
  returns a compact handoff string via the registry's ``_handoff``).
- API direct mode -> :func:`run_house_direct` (generator yielding
  serialized state snapshots, same contract as the General mode).

Backend: ``deepseek:v4-flash`` with native function calling (same transport
as the General agent).
"""

from __future__ import annotations

import json
import os
import uuid

from sync.state_sync import safe_update_state, safe_append_live_log
from agents.it_department_nodes_base import ITState
from tools import (
    execute_tool_detailed,
    run_tool_calls,
    get_native_tools,
    get_native_tool_names,
)
from llm.loop_detector import LoopGuard
from llm.model_history import build_initial_history, estimate_history_tokens
from llm.native_tools import run_native_agent
from memory.context_compaction import (
    get_compaction_policy,
)
from agents.house_prompt import _HOUSE_SYSTEM_TEMPLATE
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    SystemPromptPart,
    TextPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)

_AGENT_NAME = "House"


def _native_deps(chat_id: str):
    """Build the typed WorkspaceDeps for this run (lazy import, cheap)."""
    from llm.deps import WorkspaceDeps

    return WorkspaceDeps.from_shared_state(role=_AGENT_NAME, chat_id=chat_id)

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


def _house_context() -> str:
    """Compact live house snapshot injected into Zone 2 so the agent starts
    with real device ids/state instead of guessing."""
    try:
        import sys as _sys
        _desktop = os.path.join(_HARNESS_ROOT, "Desktop")
        if _desktop not in _sys.path:
            _sys.path.insert(0, _desktop)
        import house_manager as hm
        return f"## Current House Snapshot\n{hm.house_summary()}"
    except Exception as e:
        _log(f"[{_AGENT_NAME.upper()}] House snapshot unavailable: {e}")
        return ""


def _build_zone2(task: str, chat_id: str) -> str:
    """Volatile context: house snapshot, profile, chat history, request."""
    parts: list[str] = []

    snap = _house_context()
    if snap:
        parts.append(snap)

    if chat_id:
        try:
            from memory.chat_context import build_chat_context

            history = build_chat_context(
                "_general", chat_id, max_messages=20, max_chars_per_msg=2500, exclude_traces=True
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
    import re
    return re.sub(r"<thinking>.*?</thinking>", "", text, flags=re.DOTALL).strip()


def _native_slice_tools(new_msgs: list) -> list[dict]:
    """Extract tool calls from native run's transcript slice (ModelMessage or LC)."""
    out: list[dict] = []
    for m in new_msgs or []:
        if isinstance(m, ModelResponse):
            for p in getattr(m, "parts", []) or []:
                if isinstance(p, ToolCallPart):
                    args = p.args if isinstance(p.args, dict) else {}
                    if isinstance(args, str):
                        try:
                            args = json.loads(args)
                        except Exception:
                            args = {}
                    out.append({
                        "tool": p.tool_name,
                        "args": args if isinstance(args, dict) else {},
                        "id": p.tool_call_id or "",
                    })
            continue
        tcs = getattr(m, "tool_calls", None) or []
        for tc in tcs:
            if not isinstance(tc, dict):
                continue
            out.append({
                "tool": tc.get("name") or tc.get("tool", ""),
                "args": tc.get("args") or {},
                "id": tc.get("id") or "",
            })
    return out


# ── LLM backend ──────────────────────────────────────────────────────────────


def _call_backend(history: list, native_tools: list[dict], where_tag: str, system_prompt: str = "", deps: object = None) -> tuple[str, list[dict], list]:
    """One LLM turn. Returns ``(text, tool_calls, native_msgs)``. ``native_msgs``
    is non-empty when the native agent ran the full turn (tools executed
    internally); the caller appends them instead of executing tool_calls.
    Raises on total failure.

    Backend resolution follows the active provider mode's workspace-tier plan
    (free pool or the paid gateway path); REASONING_SUBAGENT_ROUTINE env still
    overrides the mode's thinking default.
    """
    plan: dict = {}
    try:
        from llm.provider_modes import get_role_plan

        plan = get_role_plan(_AGENT_NAME)
    except Exception:
        plan = {}

    if str(plan.get("backend")) == "free":
        from llm.free_providers import free_native_turn

        text, calls, _reasoning = free_native_turn(
            _AGENT_NAME,
            list(history),
            tools=native_tools,
            capability="text",
            prefer_reasoning=bool(plan.get("think_bias", True)),
        )
        return text, calls, []

    # Native agent (Track B): pydantic-ai executes tools internally (default).
    # A4: the legacy single-turn registry path (PAI_NATIVE_TOOLS=0) is removed;
    # the native full-turn agent is the only paid execution path.
    text, new_lc, reasoning = run_native_agent(
        role=_AGENT_NAME,
        messages=list(history),
        tools=native_tools,
        temp=0.2,
        reasoning_effort="low",  # static per-role: house=low (no ladder)
        where=where_tag,
        deps=deps,
        system_prompt=system_prompt,
    )
    return str(text or ""), [], new_lc


def _house_execute_one(tool_call: dict, deps: object = None) -> str:
    """Thread-pool worker for House's parallel read-only batch. Only
    PARALLEL_SAFE tools reach this path (enforced by run_tool_calls). Uses the
    module-level execute_tool_detailed so patching/mocking works identically to
    the sequential path."""
    tname = str(tool_call.get("tool", "") or "")
    targs = tool_call.get("args") or {}
    if not isinstance(targs, dict):
        targs = {}
    try:
        execution = execute_tool_detailed(tname, targs, deps=deps)
        return str(execution.content)
    except Exception as e:
        return f"ERROR: {str(e)[:300]}"


# ── Core loop ────────────────────────────────────────────────────────────────


def _house_events(task: str, chat_id: str = "", images: list | None = None):
    """Core agent loop as an event generator.

    Yields ``("status", line)`` events and finishes with
    ``("done", final_report)`` plus a ``("messages", new_msgs)`` event.
    """
    valid_tools = get_native_tool_names("house")
    native_tools = get_native_tools("house")
    valid_tool_set = set(valid_tools)
    # Stage 4: typed per-run deps (project/chat/role) for tool ctx injection.
    run_deps = _native_deps(chat_id)

    history: list[ModelMessage] = build_initial_history(_HOUSE_SYSTEM_TEMPLATE, _build_zone2(task, chat_id))

    if images:
        try:
            from tools.vision_channel import queue_image_file

            for p in images:
                queue_image_file(p, note="user attachment", ctx=run_deps)
        except Exception:
            pass
    try:
        from tools.vision_channel import drain_attachment_images_native

        if drain_attachment_images_native(history, ctx=run_deps):
            _log(f"[VISION] {_AGENT_NAME}: user attachment image(s) injected")
    except Exception:
        pass

    compaction_policy = get_compaction_policy("house")
    tool_call_log: list[dict] = []
    existing_len = len(history)
    iteration = 0
    final_text = ""

    yield "status", f"[{_AGENT_NAME.upper()}] Starting house task loop"

    while True:
        iteration += 1
        yield "status", f"[{_AGENT_NAME.upper()}] Iteration {iteration}"

        if chat_id:
            try:
                from inbox import consume_abort

                if consume_abort(chat_id):
                    yield "status", f"[{_AGENT_NAME.upper()}] Cancelled by user request."
                    final_text = final_text or "Cancelled by user."
                    break
            except Exception:
                pass

        if iteration > 1:
            try:
                est = estimate_history_tokens(history)
                if est >= compaction_policy.checkpoint_trigger:
                    from llm.model_history import compact_history_native

                    before = est
                    history = compact_history_native(
                        history,
                        policy=compaction_policy,
                        summary_text="[Native compaction checkpoint — earlier turns summarized]",
                    )
                    after = estimate_history_tokens(history)
                    _log(
                        f"[{_AGENT_NAME.upper()}] Native checkpoint "
                        f"({before:,} -> {after:,} est. tokens; "
                        f"tail={compaction_policy.tail_budget_tokens:,})"
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
            response_text, tool_calls, native_msgs = _call_backend(
                history, native_tools, where_tag,
                system_prompt=_HOUSE_SYSTEM_TEMPLATE,
                deps=_native_deps(chat_id),
            )
        except Exception as e:
            _log(f"[{_AGENT_NAME.upper()}] LLM error after all fallbacks: {e}")
            raise

        # Native agent ran the whole turn (tools executed internally).
        if native_msgs:
            # Native full turn always returns ModelMessage slice (P0-LLM).
            if native_msgs and isinstance(native_msgs[0], (ModelRequest, ModelResponse)):
                history.extend(native_msgs)
            else:
                try:
                    from llm.pai_transport import _messages_to_pai
                    _converted = _messages_to_pai(native_msgs)  # type: ignore
                    history.extend(_converted)
                    native_msgs = _converted
                except Exception:
                    history.extend(native_msgs)  # fallback, keep as-is
            final_text = _clean_text(response_text)
            break

        if not response_text.strip() and not tool_calls:
            history.append(ModelRequest(parts=[UserPromptPart(content=(
                    "[NUDGE] You responded with nothing. Continue the request with a tool, "
                    "or if finished, summarize what you changed.",
                ))]))
            continue

        if not tool_calls:
            final_text = _clean_text(response_text)
            history.append(ModelResponse(parts=[TextPart(content=response_text)]))
            break

        _parts: list = []
        if response_text:
            _parts.append(TextPart(content=response_text))
        for tc in tool_calls:
            _parts.append(ToolCallPart(
                tool_name=tc["tool"],
                args=tc.get("args") or {},
                tool_call_id=tc.get("id") or f"house-call-{iteration}-{uuid.uuid4().hex[:8]}",
            ))
        if _parts:
            history.append(ModelResponse(parts=_parts))

        # Parallel read-only execution: independent PARALLEL_SAFE calls run on a
        # thread pool; results return in call order. Out-of-scope tools are
        # filtered BEFORE execution (they never reach the executor). Everything
        # else stays sequential (byte-identical to the pre-parallel path).
        _allowed_calls = [c for c in tool_calls if c.get("tool") in valid_tool_set]
        _blocked_calls = [c for c in tool_calls if c.get("tool") not in valid_tool_set]
        try:
            batch_results = run_tool_calls(_allowed_calls, lambda c: _house_execute_one(c, run_deps))
        except Exception:
            batch_results = []
        _house_results_by_id = {}
        if len(batch_results) == len(_allowed_calls):
            for _tc, _res in zip(_allowed_calls, batch_results):
                _house_results_by_id[str(_tc.get("id") or "")] = _res

        for tc in tool_calls:
            tname = tc["tool"]
            targs = tc.get("args") or {}
            call_id = tc.get("id") or f"house-call-{iteration}-{len(tool_call_log)}"

            guard = LoopGuard.check_pre_execute(tool_call_log, tname, targs)

            if tname not in valid_tool_set:
                result_str = (
                    f"[BLOCKED] Tool '{tname}' is not available to {_AGENT_NAME}. "
                    f"Available: {', '.join(sorted(valid_tool_set))}"
                )
            elif str(call_id) in _house_results_by_id:
                result_str = _house_results_by_id[str(call_id)]
            else:
                try:
                    execution = execute_tool_detailed(tname, targs, deps=run_deps)
                    result_str = str(execution.content)
                except Exception as e:
                    result_str = f"ERROR: {str(e)[:300]}"

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
            history.append(ModelRequest(parts=[ToolReturnPart(tool_name=tname, content=result_str, tool_call_id=call_id)]))

        # Inline images queued by tools land as ONE synthetic user message (native).
        try:
            from tools.vision_channel import drain_into_messages_native

            if drain_into_messages_native(history, header="[Inline image(s) from tool result]", ctx=run_deps):
                _log(f"[VISION] {_AGENT_NAME} iter {iteration}: inline image(s) injected")
        except Exception:
            pass

        if chat_id:
            try:
                from inbox import drain_steering, render_delivery

                steer = drain_steering(chat_id)
                if steer:
                    history.append(ModelRequest(parts=[UserPromptPart(content=render_delivery("STEERING", steer))]))
            except Exception:
                pass

    new_msgs = history[existing_len:]
    yield "messages", new_msgs
    yield "done", final_text or "No final response produced."


def house_loop(
    task: str,
    chat_id: str = "",
    images: list | None = None,
    emit=None,
) -> str:
    """Run the House agent loop to completion. Returns the final report text."""
    new_msgs: list = []
    final_text = ""
    for kind, payload in _house_events(task, chat_id=chat_id, images=images):
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
    house_loop.last_messages = new_msgs
    return final_text


def run_house_direct(prompt: str, chat_id: str = "", images: list | None = None):
    """Generator entry point for the API server (House mode)."""
    from sync.state_sync import safe_serialize_state

    safe_update_state({
        "live_terminal_log": f"[{_AGENT_NAME}] Processing...\n",
        "outputs": {},
    })
    yield json.loads(safe_serialize_state())

    last_status = ""
    try:
        for kind, payload in _house_events(prompt, chat_id=chat_id, images=images):
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
    except Exception as e:
        import traceback

        safe_update_state({
            "error": str(e),
            "live_terminal_log": f"[{_AGENT_NAME} Error] {e}\n{traceback.format_exc()}",
        })
        yield json.loads(safe_serialize_state())


# ── Entry point ──────────────────────────────────────────────────────────────


def house_node(s: ITState) -> dict:
    """Graph/registry-facing node. Called from task('House', ...) via the
    special-case branch in subagentTools.task()."""
    client_req = s.get("client_request", "") if hasattr(s, "get") else ""
    chat_id = s.get("chat_id", "") if hasattr(s, "get") else ""
    project_path = s.get("project_path", "") if hasattr(s, "get") else ""
    images = s.get("images") if hasattr(s, "get") else None
    safe_update_state({"thoughts": {"house": "Analyzing and executing..."}})
    try:
        report = house_loop(client_req, chat_id=chat_id, images=list(images) if images else None)
        safe_update_state({
            "thoughts": {"house": "Task complete."},
            "outputs": {"agent_report": report},
        })
        return {
            "messages": list(getattr(house_loop, "last_messages", []) or []),
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
            "agent_report": f"ERROR: House agent crashed: {e}",
            "code": f"ERROR: {e}",
            "test_report": "",
            "project_path": project_path,
            "code_updated": False,
            "tech_spec_updated": False,
            "next_agent": "finish",
            "error": str(e),
        }
