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
  ``deepseek:v4.1-flash`` with native function calling.
- ``free``: routes every turn through the multi-provider FreeAgent client
  (OpenRouter/Groq/Gemini/Mistral/NVIDIA/Cloudflare/...) â€” zero cost, same
  OpenAI-native tool protocol.
"""

from __future__ import annotations

import json
import os
import re
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
from memory.context_compaction import get_compaction_policy
from agents.general_prompt import _GENERAL_SYSTEM_TEMPLATE
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

_AGENT_NAME = "General"

_HARNESS_ROOT = os.environ.get("HARNESS_ROOT") or os.path.dirname(
    os.path.dirname(os.path.abspath(__file__))
)


def _native_deps(chat_id: str):
    """Build the typed WorkspaceDeps for this run (lazy import, cheap)."""
    from llm.deps import WorkspaceDeps

    return WorkspaceDeps.from_shared_state(role=_AGENT_NAME, chat_id=chat_id)


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


# ── Context builders ───────────────────────────────────────────────────────


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
            from server.context.chat_history import build_chat_context
            from sync.state_sync import safe_get_state

            history = build_chat_context(
                "_general", chat_id, max_messages=30, max_chars_per_msg=3000,
                exclude_message_id=str(safe_get_state().get("user_message_id") or ""),
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


def _native_slice_tools(new_msgs: list) -> list[dict]:
    """Extract tool calls from native run's transcript slice (ModelMessage or LC)."""
    out: list[dict] = []
    for m in new_msgs or []:
        # Native ModelMessage path: ModelResponse with ToolCallPart
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
    return out


# ── LLM backends ───────────────────────────────────────────────────────────


def _call_backend(history: list, native_tools: list[dict], where_tag: str, system_prompt: str = "", deps: object = None, cancellation_token: object = None) -> tuple[str, list[dict], list]:
    """One LLM turn on the configured backend.

    Returns ``(text, tool_calls, native_msgs)``. ``native_msgs`` is non-empty
    when the native pydantic-ai agent ran the full turn (it executes tools
    internally); the caller appends them to the transcript instead of
    executing ``tool_calls`` itself. Raises on total provider failure.

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
            list(history),
            tools=native_tools,
            capability="text",
            prefer_reasoning=bool(plan.get("think_bias", True)),
        )
        return reply, calls, []

    # Paid path — thinking comes from the agent registry (General: low).

    # Native agent (Track B): pydantic-ai executes tools internally (default).
    # A4: the legacy single-turn registry path (PAI_NATIVE_TOOLS=0) is removed;
    # the native full-turn agent is the only paid execution path.
    text, new_lc, reasoning = run_native_agent(
        role=_AGENT_NAME,
        messages=list(history),
        tools=native_tools,
        temp=0.2,
        where=where_tag,
        deps=deps,
        system_prompt=system_prompt,
        cancellation_token=cancellation_token,
    )
    return str(text or ""), [], new_lc


# ── Session log (P1, best-effort) ───────────────────────────────────────────


def _open_session(chat_id: str, task: str):
    try:
        from uuid import uuid4

        from server.session.log import find_recent_session, get_session
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


# ── Core loop ───────────────────────────────────────────────────────────────


def _general_execute_one(tool_call: dict, deps: object = None) -> str:
    """Thread-pool worker for General's parallel read-only batch. Only
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
    # Stage 4: typed per-run deps (project/chat/role) for tool ctx injection.
    run_deps = _native_deps(chat_id)

    history: list[ModelMessage] = build_initial_history(_GENERAL_SYSTEM_TEMPLATE, _build_zone2(task, chat_id))

    # User-attached images: explicit paths first, then the API stash channel.
    # Golden standard: images land in run_deps.pending_images (typed channel).
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

    sess = _open_session(chat_id, task)

    # A8: wire the chat abort flag to a pydantic-ai CancellationToken so a
    # user abort cancels the in-flight native run cooperatively (RunCancelled).
    _cancel_token = None
    _cancel_stop = None
    try:
        if chat_id:
            from llm.native_tools import chat_cancellation_token

            _cancel_token, _cancel_stop = chat_cancellation_token(chat_id)
    except Exception:
        _cancel_token = _cancel_stop = None

    compaction_policy = get_compaction_policy("general")
    tool_call_log: list[dict] = []
    existing_len = len(history)
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

        # A6b/A7: native compaction — Codex-parity LLM handoff summary.
        if iteration > 1:
            try:
                est = estimate_history_tokens(history)
                if est >= compaction_policy.checkpoint_trigger:
                    from memory.compaction_engine import compact_history_with_summary

                    before = est
                    history, _engine_info = compact_history_with_summary(
                        history,
                        project_path="",
                        policy=compaction_policy,
                        role=_AGENT_NAME,
                        deps=run_deps,
                    )
                    after = estimate_history_tokens(history)
                    _log(
                        f"[{_AGENT_NAME.upper()}] Native checkpoint "
                        f"({before:,} -> {after:,} est. tokens; "
                        f"one_pass=True)"
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
                system_prompt=_GENERAL_SYSTEM_TEMPLATE,
                deps=_native_deps(chat_id),
                cancellation_token=_cancel_token,
            )
        except Exception as e:
            # A8: user abort -> the native run raised RunCancelled; end cleanly.
            from pydantic_ai.exceptions import RunCancelled
            if isinstance(e, RunCancelled) or "RunCancelled" in type(e).__name__:
                yield "status", f"[{_AGENT_NAME.upper()}] Cancelled by user request."
                final_text = final_text or "Cancelled by user."
                break
            _log(f"[{_AGENT_NAME.upper()}] LLM error after all fallbacks: {e}")
            raise

        # Native agent ran the whole turn (tools executed internally):
        # append its transcript slice and finish — the loop's compaction and
        # session logging still see the full history.
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
            if sess is not None:
                try:
                    sess.append_assistant(final_text, stop_reason="stop")
                except Exception:
                    pass
            break

        if not response_text.strip() and not tool_calls:
            history.append(ModelRequest(parts=[UserPromptPart(content=(
                "[NUDGE] You responded with nothing. Continue the request with a tool, "
                "or if finished, summarize what you accomplished."
            ))]))
            continue

        if not tool_calls:
            final_text = _clean_text(response_text)
            history.append(ModelResponse(parts=[TextPart(content=response_text)]))
            if sess is not None:
                try:
                    sess.append_assistant(final_text, stop_reason="stop")
                except Exception:
                    pass
            break

        # Free-pool / tool-calling path: materialize ModelResponse with ToolCallParts
        _parts: list = []
        if response_text:
            _parts.append(TextPart(content=response_text))
        for tc in tool_calls:
            _parts.append(ToolCallPart(
                tool_name=tc["tool"],
                args=tc.get("args") or {},
                tool_call_id=tc.get("id") or f"gen-call-{iteration}-{uuid.uuid4().hex[:8]}",
            ))
        if _parts:
            history.append(ModelResponse(parts=_parts))
        if sess is not None:
            try:
                sess.append_assistant(response_text, stop_reason="toolUse")
            except Exception:
                pass

        # Parallel read-only execution: independent PARALLEL_SAFE calls run on a
        # thread pool; results return in call order. Out-of-scope tools are
        # filtered BEFORE execution (they never reach the executor). Everything
        # else stays sequential (byte-identical to the pre-parallel path).
        _allowed_calls = [c for c in tool_calls if c.get("tool") in valid_tool_set]
        _blocked_calls = [c for c in tool_calls if c.get("tool") not in valid_tool_set]
        try:
            batch_results = run_tool_calls(_allowed_calls, lambda c: _general_execute_one(c, run_deps))
        except Exception:
            batch_results = []
        _gen_results_by_id = {}
        if len(batch_results) == len(_allowed_calls):
            for _tc, _res in zip(_allowed_calls, batch_results):
                _gen_results_by_id[str(_tc.get("id") or "")] = _res

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
            elif str(call_id) in _gen_results_by_id:
                result_str = _gen_results_by_id[str(call_id)]
            else:
                try:
                    execution = execute_tool_detailed(tname, targs, deps=run_deps)
                    result_str = str(execution.content)
                except Exception as e:
                    result_str = f"ERROR: {str(e)[:300]}"

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
            from tools.vision_channel import drain_into_messages_native

            drain_into_messages_native(history, header="[Inline image(s) from tool result]", ctx=run_deps)
        except Exception:
            pass

        # Steering lands at the next step boundary.
        if chat_id:
            try:
                from inbox import drain_steering, render_delivery

                steer = drain_steering(chat_id)
                if steer:
                    history.append(ModelRequest(parts=[UserPromptPart(content=render_delivery("STEERING", steer))]))
            except Exception:
                pass

    if _cancel_stop is not None:
        try:
            _cancel_stop()
        except Exception:
            pass

    new_msgs = history[existing_len:]
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

        print(f"[{_AGENT_NAME} Error] {e}\n{traceback.format_exc()}", flush=True)
        safe_update_state({
            "error": str(e),
            "live_terminal_log": f"[{_AGENT_NAME} Error] {e}",
        })
        yield json.loads(safe_serialize_state())


# ── Entry points ───────────────────────────────────────────────────────────


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
        print(f"[{_AGENT_NAME.upper()} CRASH] {e}\n{tb}", flush=True)
        _log(f"[{_AGENT_NAME.upper()} CRASH] {e}")
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
