"""Phase 0 gate assertion 3 — concurrent scheduled task runs must not cross-talk.

Regression test for the process-global report read in
``server/services/task_scheduler.py::run_task_background``.

Both runs publish their ``agent_report`` through ``sync.state_sync``. The
scoping mechanism (``active_state``) exists and is used correctly by
``agents/delegation.py:622-627``; the scheduler simply never set it. So before
the fix both threads wrote to — and read from — the root ``shared_state``, and
both posted whichever report was written last.

``07-phase-0-implementation.md`` section 8 requires this test FIRST, and it
must FAIL against the unfixed code. If it passes before the fix, the test is
wrong, not the code.

NOTE on isolation: this module deliberately does NOT import
``agents.general_agent``. Other test modules in this suite call
``tests/_util.py::tools_stub()``, which replaces ``sys.modules["tools"]`` with
a minimal stub and never restores it — so a later import of the real agent
stack fails with ``ImportError: cannot import name 'ToolExecutionResult'``.
The agent module is therefore replaced with a named double in ``sys.modules``,
which also avoids paying ~12s to import the real stack.
"""
from __future__ import annotations

import os
import sys
import threading
import types

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)

from server.services import task_scheduler as ts
from server.store.workspace_registry import GENERAL_WORKSPACE_ID
from sync.state_sync import reset_shared_state

_AGENT_MODULES = ("agents", "agents.general_agent")


def test_concurrent_task_reports_are_isolated():
    posted: dict = {}
    lock = threading.Lock()
    # Both threads must publish BEFORE either reads back, otherwise the race
    # depends on scheduling luck. The barrier makes the failure deterministic.
    barrier = threading.Barrier(2, timeout=10)

    def fake_add_message(workspace_path, chat_id, role, content, *a, **kw):
        with lock:
            posted.setdefault(chat_id, []).append((role, content))

    def fake_update_task(*a, **kw):
        return None

    def fake_run_general_direct(prompt, chat_id=None, images=None):
        from sync.state_sync import safe_update_state
        safe_update_state({"outputs": {"agent_report": "report::" + prompt}})
        barrier.wait()
        yield {}

    fake_agents = types.ModuleType("agents")
    fake_general = types.ModuleType("agents.general_agent")
    fake_general.run_general_direct = fake_run_general_direct
    fake_agents.general_agent = fake_general

    saved_modules = {name: sys.modules.get(name) for name in _AGENT_MODULES}
    saved_ts = (ts.add_message, ts.update_task)
    sys.modules["agents"] = fake_agents
    sys.modules["agents.general_agent"] = fake_general
    ts.add_message = fake_add_message
    ts.update_task = fake_update_task
    try:
        runs = [
            ("chat-A", {"id": "tA", "prompt": "A", "schedule": {}}),
            ("chat-B", {"id": "tB", "prompt": "B", "schedule": {}}),
        ]
        threads = [
            threading.Thread(
                target=ts.run_task_background,
                args=(GENERAL_WORKSPACE_ID, GENERAL_WORKSPACE_ID, chat_id, task),
                daemon=True,
            )
            for chat_id, task in runs
        ]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=20)
    finally:
        ts.add_message, ts.update_task = saved_ts
        for name, module in saved_modules.items():
            if module is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = module
        reset_shared_state()

    def reports(chat_id):
        return [c for r, c in posted.get(chat_id, []) if r == "agent"]

    got_a, got_b = reports("chat-A"), reports("chat-B")
    assert got_a == ["report::A"], (
        f"chat-A received {got_a!r}; expected ['report::A']. Concurrent task "
        f"runs cross-talk through the process-global state — see "
        f"docs/group-projects/07-phase-0-implementation.md section 5."
    )
    assert got_b == ["report::B"], (
        f"chat-B received {got_b!r}; expected ['report::B']."
    )
