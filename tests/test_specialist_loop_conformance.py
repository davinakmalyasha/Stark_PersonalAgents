"""Conformance net for the two specialist engines.

``task()`` (one-shot, tools/AddOnTool/subagentTools.py) and ``_run_turn()``
(continuable, agents/subagent_manager.py) must agree: same scripted backend
transcript in → same tool-call sequence + same final report out.

Covers every backend shape: (text, tool-call dicts, reasoning) triples,
native transcript slices, legacy LC slices (unified handling — the old
one-shot loop used to crash on these), plus a final tool-less turn.
Backends and tool execution are scripted; no LLM, no disk (fake sessions,
tmp workspace).
"""
import sys
import threading
from collections import deque
from types import SimpleNamespace

import pytest


def _ensure_real_tools():
    """tests/_util.py installs an offline stub as sys.modules['tools'].

    This suite exercises the REAL engines, so evict the stub first.
    """
    mod = sys.modules.get("tools")
    if mod is not None and not hasattr(mod, "__path__"):
        for name in [n for n in list(sys.modules)
                      if n == "tools" or n.startswith("tools.")]:
            sys.modules.pop(name, None)
    import tools  # noqa: F401


@pytest.fixture(autouse=True)
def _real_tools_env(monkeypatch, tmp_path):
    prev = sys.modules.get("tools")
    _ensure_real_tools()
    import tools as _tools

    monkeypatch.setitem(_tools.shared_state, "chat_id", "probe-conformance")
    monkeypatch.setitem(_tools.shared_state, "project_path", str(tmp_path))
    monkeypatch.setitem(_tools.shared_state, "outputs", {})
    yield
    if prev is not None and not hasattr(prev, "__path__"):
        sys.modules["tools"] = prev


class _FakeHeader:
    chat_id = "sub-probe"


class _FakeSession:
    """Stands in for SessionManager in both engines (no disk)."""

    def __init__(self):
        self.assistant_msgs = []
        self.tool_results = []
        self.statuses = []

    # task() surface
    def append_message(self, *a, **k):
        return None

    def append_assistant(self, content, stop_reason="stop"):
        self.assistant_msgs.append((content, stop_reason))

    def append_tool_result(self, *a, **k):
        self.tool_results.append((a, k))

    def get_header(self):
        return _FakeHeader()

    # _run_turn() surface
    def build_context_entries(self):
        return []

    def get_session_file(self):
        return "fake-probe.jsonl"

    def get_entries(self):
        return []

    def append_custom_entry(self, *a, **k):
        self.statuses.append((a, k))


def _scripted_backend():
    """Three turns: dict-triple with tools → native slice with a call → done."""
    from pydantic_ai.messages import (
        ModelRequest,
        ModelResponse,
        TextPart,
        ToolCallPart,
        ToolReturnPart,
    )

    _LCMessage = type("AIMessage", (), {})

    def _lc_msg(text, call_id, tool_name):
        m = _LCMessage()
        m.content = text
        m.tool_calls = [{"id": call_id, "name": tool_name,
                         "args": {"file_path": "c.py"}}]
        return m

    turns = iter([
        ("working on it", [
            {"tool": "read_file", "args": {"file_path": "a.py"}, "id": "t1"},
            {"tool": "search_code", "args": {"pattern": "foo"}, "id": "t2"},
        ], None),
        ("checking b", [
            ModelResponse(parts=[
                TextPart(content="checking"),
                ToolCallPart(tool_name="read_file",
                             args={"file_path": "b.py"},
                             tool_call_id="t3"),
            ]),
            ModelRequest(parts=[
                ToolReturnPart(tool_name="read_file",
                               content="b-contents",
                               tool_call_id="t3"),
            ]),
        ], None),
        ("checking c", [_lc_msg("checking c", "t4", "search_code")], None),
        ("all done", [], None),
    ])

    calls = []

    def _backend(name, messages, **kwargs):
        calls.append(name)
        return next(turns)

    _backend.calls = calls
    return _backend


def _recording_execute(calls):
    def _execute(name, args, deps=None):
        calls.append((name, dict(args or {})))
        return SimpleNamespace(content=f"[{name} output]")
    return _execute


def _patch_common(monkeypatch, backend, executed):
    import tools as _tools
    import tools.AddOnTool.subagentTools as st

    monkeypatch.setattr(st, "_specialist_call_backend", backend)
    monkeypatch.setattr(_tools, "execute_tool_detailed",
                        _recording_execute(executed))


def test_loops_agree_on_scripted_transcript(monkeypatch):
    import tools.AddOnTool.subagentTools as st
    import server.session.log as _slog
    from agents import subagent_manager as sm

    # task(): fake the child session log (no disk).
    task_session = _FakeSession()
    monkeypatch.setattr(_slog, "get_session", lambda *a, **k: task_session)
    monkeypatch.setattr(_slog, "find_recent_session", lambda *a, **k: None)

    backend_a = _scripted_backend()
    executed_a = []
    _patch_common(monkeypatch, backend_a, executed_a)
    result_task = st.task(name="Explore", task="probe task")

    backend_b = _scripted_backend()
    executed_b = []
    _patch_common(monkeypatch, backend_b, executed_b)
    import agents.workspace_registry as _reg  # noqa: F401  (warms specialist specs)
    act = {
        "child_id": "cont-probe",
        "status": "idle",
        "queue": deque(),
        "interrupt": False,
        "lock": threading.RLock(),
        "session_path": None,
        "name": "Explore",
        "parent_chat_id": "",
        "workspace": "",
        "last_result": "",
    }
    cont_session = _FakeSession()
    result_turn = sm._run_turn(act, cont_session, "Explore", extra_messages=[])

    # Same backend consumption: 4 scripted turns each.
    assert backend_a.calls == ["Explore"] * 4
    assert backend_b.calls == ["Explore"] * 4
    # Same tool sequence (turn-2 native slice carries its own result).
    assert sorted(n for n, _ in executed_a) == ["read_file", "search_code"]
    assert sorted(n for n, _ in executed_b) == ["read_file", "search_code"]
    # Same final report.
    assert result_task == "all done"
    assert result_turn == "all done"
    # Both engines logged assistant + tool activity to their sessions.
    assert len(task_session.assistant_msgs) >= 2
    assert len(cont_session.assistant_msgs) >= 2
    assert cont_session.statuses, "continuable turn must record idle status"
