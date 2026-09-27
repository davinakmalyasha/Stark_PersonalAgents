"""Specialist turn mechanics — the single home for the specialist loop.

One loop (``run_specialist_loop``) serves both lifecycle shells: the one-shot
``task()`` path in ``tools/AddOnTool/subagentTools.py`` and the continuable
``_run_turn()`` in ``agents/subagent_manager.py``. Shells own transcript
assembly, session persistence, and epilogues; everything that happens
*inside* an iteration lives here exactly once, steered by ``LoopHooks``.
``tests/test_specialist_loop_conformance.py`` replays a scripted transcript
through both shells and asserts they agree.

All imports stay function-local (this module is imported by both sides of
the agents/tools boundary).
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Optional


def is_native_slice(second: list) -> bool:
    """True when a backend result's 2nd element is a transcript slice.

    Handles both legacy LC (AIMessage/ToolMessage — test compat) and native
    ModelMessage (ModelResponse/ModelRequest) slices. A plain list of
    tool-call dicts (free-pool triple) returns False. Byte-identical
    behavior to the two copies this replaces.
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


def specialist_turn_deps(name: str, chat_id: str = ""):
    """Per-turn WorkspaceDeps for a specialist (role + chat), or None.

    Values mirror shared_state; only the role identity is added, so
    scope-aware tools (update_plan, vision) resolve the specialist instead
    of falling back to the caller. Never raises.
    """
    try:
        from llm.deps import WorkspaceDeps
        return WorkspaceDeps.from_shared_state(role=name, chat_id=chat_id or "")
    except Exception:
        return None


@dataclass
class SpecialistLoopHooks:
    """Policy knobs distinguishing the lifecycle shells.

    - ``id_prefix``: tool-call id namespace ("sub" one-shot, "cont" durable).
    - ``parallel``: run PARALLEL_SAFE calls through the thread pool (one-shot
      only; durable turns stay sequential so the session log matches execution).
    - ``schema``: structured-output schema or None (one-shot only).
    - ``check_interrupt``: cooperative cancel flag (durable only).
    - ``raise_backend_errors``: propagate backend failures (one-shot) or
      record them as an ERROR result (durable).
    - ``note_vision``: log a system note when inline images are injected.
    - ``scope_filter``: None = execute everything; else return a skip message.
    - ``classify_error(result, raised)``: session-log error flag rule.
    - ``log_assistant`` / ``log_tool_result`` / ``log_system``: session sinks
      (None-tolerant; all calls are best-effort).
    - ``append_final`` / ``append_with_calls``: transcript-tail construction,
      preserved verbatim per shell (provider-visible bytes).
    - ``compact``: free-path transcript checkpoint.
    - ``deps``: per-turn WorkspaceDeps for execution + vision drains.
    """

    id_prefix: str = "sub"
    parallel: bool = False
    schema: Any = None
    check_interrupt: Optional[Callable[[], bool]] = None
    raise_backend_errors: bool = True
    note_vision: bool = False
    scope_filter: Optional[Callable[[dict], Optional[str]]] = None
    classify_error: Optional[Callable[[str, bool], bool]] = None
    log_assistant: Optional[Callable[[list, str], None]] = None
    log_tool_result: Optional[Callable[[str, str, str, bool], None]] = None
    log_system: Optional[Callable[[str], None]] = None
    append_final: Optional[Callable[[list, str, Any], None]] = None
    append_with_calls: Optional[Callable[[list, str, Any, list], None]] = None
    compact: Optional[Callable[[list], None]] = None
    deps: Any = None


@dataclass
class SpecialistLoopOutcome:
    content: str = ""
    interrupted: bool = False
    backend_error: str = ""


def run_specialist_loop(
    *,
    name: str,
    transcript: list,
    backend_fn,
    execute_fn,
    hooks: SpecialistLoopHooks,
) -> SpecialistLoopOutcome:
    """Run specialist turns to completion on ``transcript`` (mutated in place).

    ``backend_fn(messages, iteration, last_tool)`` performs one model turn.
    ``execute_fn(call)`` runs one in-scope tool call and returns
    ``(result_text, log_text, raised)``. Every lifecycle divergence between
    the one-shot and durable shells is a ``hooks`` policy; the iteration
    skeleton below is shared exactly.
    """
    from pydantic_ai.messages import (
        ModelRequest,
        ModelResponse,
        TextPart,
        ToolCallPart,
        ToolReturnPart,
    )

    def _interrupted() -> bool:
        try:
            return bool(hooks.check_interrupt and hooks.check_interrupt())
        except Exception:
            return False

    def _emit_assistant(blocks, stop_reason) -> None:
        try:
            if hooks.log_assistant is not None:
                hooks.log_assistant(blocks, stop_reason)
        except Exception:
            pass

    def _emit_tool_result(content, call_id, tool_name, is_error) -> None:
        try:
            if hooks.log_tool_result is not None:
                hooks.log_tool_result(content, call_id, tool_name, is_error)
        except Exception:
            pass

    def _emit_system(text) -> None:
        try:
            if hooks.log_system is not None:
                hooks.log_system(text)
        except Exception:
            pass

    def _classify(result_text, raised) -> bool:
        try:
            if hooks.classify_error is not None:
                return bool(hooks.classify_error(result_text, raised))
        except Exception:
            pass
        return bool(raised)

    content = ""
    backend_error = ""
    interrupted = False
    last_tool = ""
    i = 0
    while True:
        i += 1
        if _interrupted():
            interrupted = True
            break
        if hooks.compact is not None:
            try:
                hooks.compact(transcript)
            except Exception:
                pass
        try:
            res = backend_fn(list(transcript), i, last_tool)
        except Exception as e:
            if hooks.raise_backend_errors:
                raise
            backend_error = f"{type(e).__name__}: {e}"
            _emit_system(f"[ERROR] {backend_error}")
            break

        reasoning = getattr(res, "reasoning_content", None)
        # Native full turn — the backend executed the tools internally; the
        # slice IS the execution record.
        if isinstance(res, tuple) and len(res) == 3 and is_native_slice(res[1]):
            response_text, slice_msgs, reasoning = res
            try:
                native = bool(
                    slice_msgs
                    and isinstance(slice_msgs[0], (ModelRequest, ModelResponse))
                )
            except Exception:
                native = False
            if native:
                transcript.extend(slice_msgs)
                slice_calls: list = []
                for m in slice_msgs:
                    if isinstance(m, ModelResponse):
                        for p in getattr(m, "parts", []) or []:
                            if isinstance(p, ToolCallPart):
                                args = p.args if isinstance(p.args, dict) else {}
                                if isinstance(args, str):
                                    try:
                                        import json as _json

                                        args = _json.loads(args)
                                    except Exception:
                                        args = {}
                                slice_calls.append({
                                    "id": p.tool_call_id or "",
                                    "name": p.tool_name or "",
                                    "args": args if isinstance(args, dict) else {},
                                })
            else:
                # Legacy LC slice (test compat) — convert to native for history.
                try:
                    from llm.pai_transport import _messages_to_pai

                    transcript.extend(_messages_to_pai(slice_msgs))  # type: ignore
                except Exception:
                    try:
                        transcript.extend(slice_msgs)  # type: ignore
                    except Exception:
                        pass
                slice_calls = [
                    {"id": tc.get("id", ""), "name": tc.get("name", ""),
                     "args": tc.get("args") or {}}
                    for m in slice_msgs
                    if m.__class__.__name__ == "AIMessage"
                    for tc in (getattr(m, "tool_calls", None) or [])
                ]
            blocks = [{"type": "text", "text": str(response_text)}]
            if reasoning:
                blocks.insert(0, {"type": "thinking", "thinking": str(reasoning)})
            _emit_assistant(blocks, "toolUse" if slice_calls else "stop")
            if not slice_calls:
                content = str(response_text)
                break
            for tc in slice_calls:
                _emit_tool_result("ok", tc.get("id", ""), tc.get("name", ""),
                                  False)
            last_tool = slice_calls[-1].get("name", "")
            continue

        # Structured output (one-shot shell only): the validated payload IS
        # the result.
        if hooks.schema is not None and hasattr(res, "model_dump_json"):
            content = res.model_dump_json()
            transcript.append(ModelResponse(parts=[TextPart(content=content)]))
            _emit_assistant(content, "stop")
            break
        if hooks.schema is not None and hasattr(res, "json"):
            content = res.json()
            transcript.append(ModelResponse(parts=[TextPart(content=content)]))
            _emit_assistant(content, "stop")
            break

        # Native triple: (text, tool calls, reasoning).
        native_tool_calls: list = []
        if isinstance(res, tuple):
            response_text = res[0] if len(res) > 0 else ""
            native_tool_calls = res[1] if len(res) > 1 else []
            if len(res) > 2:
                reasoning = res[2]
        else:
            response_text = getattr(res, "content", str(res))
        response_text = str(response_text)

        tool_calls = []
        for call_index, tc in enumerate(native_tool_calls or []):
            tname = tc.get("tool") or tc.get("name", "")
            if tname:
                tool_calls.append({
                    "tool": tname,
                    "args": tc.get("args") or {},
                    "id": tc.get("id") or f"{hooks.id_prefix}-{i}-{call_index + 1}",
                })

        if not tool_calls:
            if hooks.append_final is not None:
                hooks.append_final(transcript, response_text, reasoning)
            blocks = [{"type": "text", "text": response_text}]
            if reasoning:
                blocks.insert(0, {"type": "thinking", "thinking": str(reasoning)})
            _emit_assistant(blocks, "stop")
            content = response_text
            break

        if hooks.append_with_calls is not None:
            hooks.append_with_calls(transcript, response_text, reasoning,
                                    tool_calls)
        blocks = [{"type": "text", "text": response_text}]
        if reasoning:
            blocks.insert(0, {"type": "thinking", "thinking": str(reasoning)})
        _emit_assistant(blocks, "toolUse")
        last_tool = tool_calls[-1].get("tool", "")

        # Execute: parallel pool (one-shot) or sequential with per-call
        # interrupt checks (durable). Each execution yields
        # (transcript_text, log_text, raised).
        def _guarded_execute(tc):
            try:
                skip = hooks.scope_filter(tc) if hooks.scope_filter else None
            except Exception:
                skip = None
            if skip is not None:
                return (skip, skip, False)
            try:
                return execute_fn(tc)
            except Exception as e:  # worker bugs must not kill the loop
                return (f"ERROR: {e}", f"ERROR: {e}", True)

        executed: list = []
        if hooks.parallel:
            def _single(tc):
                return _guarded_execute(tc)[0]
            try:
                from tools import run_tool_calls as _run_pool

                pooled = _run_pool(tool_calls, _single)
            except Exception:
                pooled = []
            by_id = {}
            if len(pooled) == len(tool_calls):
                for _tc, _res in zip(tool_calls, pooled):
                    by_id[str(_tc.get("id") or "")] = _res
            for tc in tool_calls:
                cid = str(tc.get("id") or "")
                if cid in by_id:
                    s = by_id[cid]
                    executed.append((s, s, False))
                else:
                    executed.append(_guarded_execute(tc))
        else:
            for tc in tool_calls:
                if _interrupted():
                    interrupted = True
                    break
                executed.append(_guarded_execute(tc))
        if interrupted:
            break
        for tc, (t_text, l_text, raised) in zip(tool_calls, executed):
            _emit_tool_result(l_text, tc["id"], tc["tool"],
                              _classify(t_text, raised))
            transcript.append(ModelRequest(parts=[ToolReturnPart(
                tool_name=tc.get("tool", ""),
                content=str(t_text),
                tool_call_id=tc["id"],
            )]))

        # Vision: drain tool-queued inline images into ONE user message.
        try:
            from tools.vision_channel import drain_into_messages_native

            if drain_into_messages_native(
                transcript,
                header="[Inline image(s) from tool result]",
                ctx=hooks.deps,
            ):
                if hooks.note_vision:
                    _emit_system("[VISION] inline image(s) injected")
        except Exception:
            pass

    return SpecialistLoopOutcome(
        content=content, interrupted=interrupted, backend_error=backend_error,
    )
