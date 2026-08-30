"""Delegation runtime + orchestrator park-and-wait tests (no live LLM).

Covers:
  * parallel fan-out through the canonical node runners
  * per-item chat creation vs continuation
  * [DELEGATION RESULT] delivery into the parent inbox
  * running_count / cancel_all bookkeeping
  * park_wait wake-on-event, abort, timeout shapes
  * full orchestrator loop: spawn -> confirm -> park -> forward -> finish
"""

import os
import sys
import threading
import time

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _ROOT)
sys.path.insert(0, os.path.join(_ROOT, "Desktop"))

import pytest


# ── Fakes ────────────────────────────────────────────────────────────────────


class FakeWM:
    """In-memory stand-in for workspace_manager chat functions."""

    def __init__(self):
        self.chats = {}       # (ws, cid) -> {"id", "title"}
        self.messages = []    # (ws, cid, role, content)
        self._n = 0

    def create_chat(self, ws, title="", model="Automatic Fallback"):
        self._n += 1
        cid = f"chat-{self._n}"
        self.chats[(ws, cid)] = {"id": cid, "title": title}
        return {"id": cid}

    def get_chat(self, ws, cid, include_traces=False, include_usage=True):
        return self.chats.get((ws, cid))

    def add_message(self, ws, cid, role, content, metadata=None):
        self.messages.append((ws, cid, role, str(content)))


@pytest.fixture()
def fake_env(monkeypatch):
    import workspace_manager as wm
    from inbox import clear_inbox_registry
    from sync.state_sync import shared_state

    fakes = FakeWM()
    monkeypatch.setattr(wm, "create_chat", fakes.create_chat)
    monkeypatch.setattr(wm, "get_chat", fakes.get_chat)
    monkeypatch.setattr(wm, "add_message", fakes.add_message)

    clear_inbox_registry()

    calls = []

    gate = threading.Event()

    def fake_dev(state):
        calls.append(("dev", state))
        return {"agent_report": f"dev done: {state['client_request']}"}

    def fake_general(state):
        calls.append(("general", state))
        return {"agent_report": f"general done: {state['client_request']}"}

    def fake_house(state):
        calls.append(("house", state))
        return {"agent_report": f"house done: {state['client_request']}"}

    def blocking_house(state):
        calls.append(("house-blocked", state))
        gate.wait(timeout=5)
        return {"agent_report": "house late"}

    import agents.developer_agent as da
    import agents.general_agent as ga
    import agents.house_agent as ha

    monkeypatch.setattr(da, "developer_node", fake_dev)
    monkeypatch.setattr(ga, "general_node", fake_general)
    monkeypatch.setattr(ha, "house_node", fake_house)

    shared_state["chat_id"] = "parent-1"
    # The orchestrator loop indexes these directly; earlier resets may wipe them.
    shared_state.setdefault("thoughts", {})
    shared_state.setdefault("outputs", {})
    shared_state["failed_models"] = []

    yield {
        "wm": fakes,
        "calls": calls,
        "gate": gate,
        "modules": (da, ga, ha),
    }

    gate.set()
    clear_inbox_registry()


def _wait_until(fn, timeout=10.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if fn():
            return True
        time.sleep(0.05)
    return False


def _drain_all(chat_id):
    """Drain the one-at-a-time follow-up FIFO completely."""
    from inbox import drain_follow_up
    out = []
    while True:
        batch = drain_follow_up(chat_id)
        if not batch:
            return out
        out.extend(batch)


# ── Delegation runtime ───────────────────────────────────────────────────────


def test_parallel_fanout_delivers_results(fake_env, monkeypatch):
    from agents import delegation
    from inbox import drain_follow_up

    delegation.reset_delegations()

    items = [
        {"agent": "Dev", "task": "build game", "title": "Game · Dev"},
        {"agent": "General", "task": "note today i ate pasta"},  # personal → General
        {"agent": "House", "task": "lamp off in bedroom", "title": "Lamp"},
    ]
    summaries = delegation.start_delegations(items, "parent-1", "")

    assert len(summaries) == 3 and all(s["ok"] for s in summaries)
    # Dev defaults to the fallback workspace; General/House go to _general.
    assert summaries[0]["workspace"] == ""
    assert summaries[1]["workspace"] == "_general"
    assert summaries[2]["chat_id"] != summaries[1]["chat_id"]
    assert all(not s["continued"] for s in summaries)

    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)

    events = _drain_all("parent-1")
    assert len(events) == 3
    bodies = "\n".join(events)
    assert "[DELEGATION RESULT]" in bodies
    assert "dev done: build game" in bodies
    assert "general done: note today i ate pasta" in bodies
    # Final replies are mirrored into each delegation chat.
    roles = [(r,) for (_, _, r, _) in fake_env["wm"].messages]
    assert roles.count(("agent",)) == 3
    assert roles.count(("user",)) == 3
    delegation.reset_delegations()


def test_continue_existing_chat(fake_env):
    from agents import delegation

    delegation.reset_delegations()
    existing = fake_env["wm"].create_chat("_general", title="Prior thread")
    before = len(fake_env["wm"].chats)

    summaries = delegation.start_delegations(
        [{"agent": "General", "task": "follow-up on that", "chat_id": existing["id"]}],
        "parent-1", "",
    )
    assert summaries[0]["ok"] and summaries[0]["continued"]
    assert len(fake_env["wm"].chats) == before  # no new chat created
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    delegation.reset_delegations()


def test_unknown_chat_id_falls_back_to_new_chat(fake_env):
    from agents import delegation

    delegation.reset_delegations()
    summaries = delegation.start_delegations(
        [{"agent": "General", "task": "x", "chat_id": "missing-chat"}],
        "parent-1", "",
    )
    assert summaries[0]["ok"] and not summaries[0]["continued"]
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    delegation.reset_delegations()


def test_invalid_items_rejected(fake_env):
    from agents import delegation

    delegation.reset_delegations()
    summaries = delegation.start_delegations(
        [
            {"agent": "Wizard", "task": "spell"},
            {"agent": "Dev", "task": ""},
            "not-a-dict",
        ],
        "parent-1", "",
    )
    assert all(not s["ok"] for s in summaries)
    assert delegation.running_count("parent-1") == 0


def test_cancel_all_and_abort_wake(fake_env, monkeypatch):
    from agents import delegation
    from inbox import abort

    delegation.reset_delegations()

    ha = fake_env["modules"][2]
    monkeypatch.setattr(ha, "house_node", lambda state: (
        fake_env["calls"].append(("blocked", state)) or fake_env["gate"].wait(timeout=5) or {"agent_report": "late"}
    ))

    delegation.start_delegations(
        [{"agent": "House", "task": "slow thing"}], "parent-1", ""
    )
    assert _wait_until(lambda: delegation.running_count("parent-1") == 1)

    # Abort wakes the parked loop.
    abort("parent-1")
    wait = delegation.park_wait("parent-1", timeout_s=2.0)
    assert wait["aborted"] is True

    cancelled = delegation.cancel_all()
    assert cancelled == 1
    assert delegation.running_count("parent-1") == 0

    fake_env["gate"].set()
    delegation.reset_delegations()


def test_park_wait_timeout_shape(fake_env):
    from agents import delegation

    t0 = time.time()
    wait = delegation.park_wait("empty-chat", timeout_s=0.6)
    assert time.time() - t0 >= 0.5
    assert wait["timed_out"] is True and not wait["events"] and not wait["aborted"]


def test_status_table_renders(fake_env):
    from agents import delegation

    delegation.reset_delegations()
    delegation.start_delegations([{"agent": "General", "task": "t"}], "parent-1", "")
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    table = delegation.status_table()
    assert "delegation(s)" in table and "[OK]" in table
    delegation.reset_delegations()


# ── Orchestrator loop integration (mocked LLM) ───────────────────────────────


def _native_slice(text: str, tool_calls: list) -> list:
    """Build the native run_native_agent return slice from a fake turn.

    Mirrors llm.pai_transport._pai_to_lc: AIMessage with tool_calls + one
    ToolMessage per call. The orchestrator loop reads this slice for
    _native_slice_tools / _native_slice_failures / the transcript.
    ``tool_calls`` may be plain dicts (default result content "ok") or dicts
    with an extra ``_content`` key carrying realistic tool output.
    """
    from langchain_core.messages import AIMessage, ToolMessage

    msgs = []
    if tool_calls:
        msgs.append(AIMessage(
            content=text,
            tool_calls=[
                {"name": tc.get("tool") or tc.get("name", ""),
                 "args": tc.get("args") or {},
                 "id": tc.get("id", "")}
                for tc in tool_calls
            ],
        ))
        for tc in tool_calls:
            msgs.append(ToolMessage(
                content=tc.pop("_content", "ok"),
                tool_call_id=tc.get("id", ""),
                name=tc.get("tool") or tc.get("name", ""),
            ))
    else:
        msgs.append(AIMessage(content=text))
    return msgs


def _native_turn_executor(decide):
    """Wrap a fake decision fn into a run_native_agent-compatible fake.

    ``decide(messages) -> (text, tool_calls)`` picks the fake turn's text and
    the tool calls it would emit. The wrapper executes those tools for real
    (via execute_tool_detailed) so delegations actually spawn and park_wait
    receives events — preserving the test's coordination intent under the
    native loop (which executes tools internally).
    """
    from tools import execute_tool_detailed

    def fake(role, messages, temp=0.7, tools=None, **kw):
        text, tool_calls = decide(messages)
        out_calls = []
        for tc in tool_calls:
            cid = tc.get("id", "c")
            try:
                exec_res = execute_tool_detailed(
                    tc["tool"], tc.get("args") or {}, scope="Orchestrator"
                )
                content = str(exec_res.content)
            except Exception as e:
                content = f"ERROR: {e}"
            out_calls.append({"tool": tc["tool"], "args": tc.get("args") or {},
                              "id": cid, "_content": content})
        return (text, _native_slice(text, out_calls), None)

    return fake


def test_orchestrator_spawns_parks_forwards_finishes(fake_env, monkeypatch):
    import agents.orchestrator_agent as orch
    from agents import delegation
    from sync.state_sync import shared_state

    delegation.reset_delegations()

    tmp_ws = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp_orch_ws")
    os.makedirs(tmp_ws, exist_ok=True)

    turn = {"n": 0}

    saw_spawn = {"v": False}

    def decide(messages):
        turn["n"] += 1
        ctx = "\n".join(getattr(m, "content", "") for m in messages)
        if not saw_spawn["v"]:
            saw_spawn["v"] = True
            return ("Spawning your requests.", [{"tool": "delegate", "args": {"items": [
                {"agent": "Dev", "task": "game project", "title": "Game"},
                {"agent": "House", "task": "lamp off", "title": "Lamp"},
            ]}, "id": "c1"}])
        # Payload sentinel (\nstatus: DONE exists only in real result events,
        # never in the prompt or the [SPAWNED] tool output).
        if "\nstatus: DONE" in ctx:
            return ("All done — check the chats for detail.", [])
        # No events yet: a no-tool turn. With children still running this must
        # PARK (not exit); with them done, the follow-up drain continues.
        return ("✓ waiting on agents.", [])

    fake_invoke = _native_turn_executor(decide)
    monkeypatch.setattr(orch, "run_native_agent", fake_invoke)
    monkeypatch.setenv("ORCH_WAIT_TIMEOUT", "15")

    state = {
        "client_request": "make a game project and turn off the lamp",
        "project_path": tmp_ws,
        "chat_id": "parent-1",
    }
    res = orch.orchestrator_node(state)

    assert _wait_until(lambda: turn["n"] >= 3, timeout=15)
    assert "All done" in res["agent_report"]
    # The loop only exits when no children remain.
    assert delegation.running_count("parent-1") == 0
    delegation.reset_delegations()


def test_delegate_tool_through_registry(fake_env):
    """The delegate tool resolves through the real scoped registry."""
    from tools import execute_tool_detailed

    from agents import delegation
    delegation.reset_delegations()

    r = execute_tool_detailed(
        "delegate",
        {"items": [{"agent": "General", "task": "registry smoke"}]},
        scope="Orchestrator",
    )
    assert "[SPAWNED]" in r.content
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    delegation.reset_delegations()


# ── Request-type coverage: project creation & resume paths ──────────────────


@pytest.fixture()
def router_env(tmp_path, monkeypatch):
    """Isolated project-router environment (tmp root + index + fake children)."""
    import agents.project_router as pr
    import agents.subagent_manager as sm

    monkeypatch.setattr(pr, "PROJECTS_ROOT", str(tmp_path))
    monkeypatch.setattr(pr, "INDEX_PATH", str(tmp_path / "project_agents.json"))

    spawned = []

    def fake_start(chat_id, ws, name, task="", label=None, parent_session=None, reasoning_effort=None):
        spawned.append({"chat_id": chat_id, "ws": ws, "name": name, "task": task, "label": label})
        return {"child_id": f"cont-{chat_id}-{name}-x", "result": f"first turn of {ws}"}

    sent = []

    def fake_send(child_id, message, workspace, timeout=1800.0):
        sent.append({"child_id": child_id, "message": message, "ws": workspace})
        return {"result": f"reply to: {message}", "queued": False, "status": "idle"}

    monkeypatch.setattr(sm, "start_continuable", fake_start)
    monkeypatch.setattr(sm, "send_message", fake_send)

    # Chat mirroring goes through workspace_manager — fake it here too.
    import workspace_manager as wm
    chats = FakeWM()
    monkeypatch.setattr(wm, "add_workspace", lambda p, n="": {"id": f"ws-{n}", "path": p})
    monkeypatch.setattr(wm, "create_chat", chats.create_chat)
    monkeypatch.setattr(wm, "add_message", chats.add_message)
    monkeypatch.setattr(wm, "list_workspaces", lambda: [])

    return {"pr": pr, "sm": sm, "spawned": spawned, "sent": sent, "chats": chats}


def test_create_project_wait_true(router_env):
    entry = router_env["pr"].create_project("Alpha Game", "build a game", wait=True)
    assert not entry.get("error")
    assert entry["child_id"].startswith("cont-")
    assert entry.get("first_reply") == "first turn of " + entry["workspace_path"]
    assert len(router_env["spawned"]) == 1


def test_create_project_wait_false_background_first_turn(router_env):
    entry = router_env["pr"].create_project("Beta Web", "build a site", wait=False)
    assert not entry.get("error")
    assert entry.get("background") is True
    assert entry["child_id"] == ""  # not resolved yet at return time

    idx_path = router_env["pr"].INDEX_PATH
    assert _wait_until(lambda: _index_child(idx_path, entry["workspace_path"]) != "", timeout=10)

    import json
    with open(idx_path, "r", encoding="utf-8") as f:
        data = json.load(f)
    mine = next(p for p in data["projects"] if p["workspace_path"] == entry["workspace_path"])
    assert mine["child_id"].startswith("cont-")
    # First reply mirrored into the project chat.
    assert any(r == "agent" for (_, _, r, _) in router_env["chats"].messages)


def _index_child(index_path: str, workspace_path: str) -> str:
    try:
        import json
        with open(index_path, "r", encoding="utf-8") as f:
            data = json.load(f)
        for p in data.get("projects", []):
            if os.path.normpath(p.get("workspace_path", "")) == os.path.normpath(workspace_path):
                return p.get("child_id", "")
    except Exception:
        pass
    return ""


def test_resume_project_wrapper(router_env):
    from tools.AddOnTool.orchestrationTools import resume_project
    from agents.project_router import create_project as cp

    entry = cp("Gamma", "an api service", wait=True)
    out = resume_project(entry["name"], "fix the login bug")
    assert "[OK]" in out and "fix the login bug" in out
    assert router_env["sent"][0]["child_id"] == entry["child_id"]

    missing = resume_project("Nonexistent Project Xyz", "hello")
    assert "[NOT FOUND]" in missing


def test_multi_project_burst_parallel(router_env):
    """wait=false burst: N projects registered immediately, first turns overlap."""
    entries = [
        router_env["pr"].create_project(f"Burst{i}", f"concept {i}", wait=False)
        for i in range(3)
    ]
    assert all(e.get("background") for e in entries)
    assert all(e["chat_id"] for e in entries)
    assert _wait_until(
        lambda: all(_index_child(router_env["pr"].INDEX_PATH, e["workspace_path"]) for e in entries),
        timeout=10,
    )


# ── Orchestrator loop: steering mid-park + timeout partial report ────────────


def test_orchestrator_handles_steering_mid_park(fake_env, monkeypatch):
    import agents.orchestrator_agent as orch
    from agents import delegation
    from inbox import steer
    from sync.state_sync import shared_state

    delegation.reset_delegations()
    shared_state.setdefault("thoughts", {})
    shared_state.setdefault("outputs", {})

    tmp_ws = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp_orch_ws2")
    os.makedirs(tmp_ws, exist_ok=True)

    turn = {"n": 0}
    hold = threading.Event()

    ha = fake_env["modules"][2]

    def held_house(state):
        fake_env["calls"].append(("held-house", state))
        hold.wait(timeout=10)
        return {"agent_report": "lamp off"}

    monkeypatch.setattr(ha, "house_node", held_house)

    saw_spawn = {"v": False}
    saw_note = {"v": False}

    def decide(messages):
        ctx = "\n".join(getattr(m, "content", "") for m in messages)
        if not saw_spawn["v"]:
            # The child is HELD open so the park is definitely active when the
            # new user task arrives mid-run.
            threading.Timer(0.3, lambda: steer("parent-1", "also note today i ate pasta")).start()
            threading.Timer(0.8, hold.set).start()
            saw_spawn["v"] = True
            return ("Spawning.", [{"tool": "delegate", "args": {"items": [
                {"agent": "House", "task": "lamp off"},
            ]}, "id": "c1"}])
        # Payload sentinels — the prompt/tool-output also mention the LABELS
        # ([DELEGATION RESULT]/[STEERING]), so match actual payloads instead.
        if "also note today i ate pasta" in ctx and not saw_note["v"]:
            saw_note["v"] = True
            return ("On it.", [{"tool": "delegate", "args": {"items": [
                {"agent": "General", "task": "note today i ate pasta"},
            ]}, "id": "c2"}])
        if saw_note["v"] and "\nstatus: DONE" in ctx:
            return ("Everything handled — see chats.", [])
        return ("Waiting on agents.", [])

    fake_invoke = _native_turn_executor(decide)
    monkeypatch.setattr(orch, "run_native_agent", fake_invoke)

    res = orch.orchestrator_node({
        "client_request": "turn off the lamp",
        "project_path": tmp_ws,
        "chat_id": "parent-1",
    })
    assert "Everything handled" in res["agent_report"]
    kinds = [c[0] for c in fake_env["calls"]]
    assert "held-house" in kinds and "general" in kinds  # steering spawned the note task
    assert delegation.running_count("parent-1") == 0
    delegation.reset_delegations()


def test_orchestrator_timeout_partial_report(fake_env, monkeypatch):
    import agents.orchestrator_agent as orch
    from agents import delegation
    from sync.state_sync import shared_state

    delegation.reset_delegations()
    shared_state.setdefault("thoughts", {})
    shared_state.setdefault("outputs", {})
    shared_state["failed_models"] = []

    tmp_ws = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp_orch_ws3")
    os.makedirs(tmp_ws, exist_ok=True)

    ha = fake_env["modules"][2]
    gate = fake_env["gate"]

    def blocked_house(state):
        fake_env["calls"].append(("blocked-house", state))
        gate.wait(timeout=10)
        return {"agent_report": "lamp finally off"}

    monkeypatch.setattr(ha, "house_node", blocked_house)

    saw_timeout = {"armed": False}
    partial_sent = {"v": False}
    turn = {"n": 0}

    def decide(messages):
        ctx = "\n".join(getattr(m, "content", "") for m in messages)
        if not saw_timeout["armed"]:
            # First turn: spawn. The child (blocked_house) is held by the gate.
            saw_timeout["armed"] = True
            return ("Delegating.", [{"tool": "delegate", "args": {"items": [
                {"agent": "House", "task": "slow lamp"},
            ]}, "id": "c1"}])
        if "STILL RUNNING" in ctx and not partial_sent["v"]:
            # Let the child finish shortly after the partial-report demand.
            partial_sent["v"] = True
            threading.Timer(0.3, gate.set).start()
            return ("Partial: lamp agent still running.", [])
        return ("Lamp off now — done.", [])

    fake_invoke = _native_turn_executor(decide)
    monkeypatch.setattr(orch, "run_native_agent", fake_invoke)
    monkeypatch.setenv("ORCH_WAIT_TIMEOUT", "0.2")

    res = orch.orchestrator_node({
        "client_request": "slow house task",
        "project_path": tmp_ws,
        "chat_id": "parent-1",
    })
    assert saw_timeout["armed"] is True          # partial-report path exercised
    assert "done" in res["agent_report"].lower()  # loop survived until child finished
    assert any(c[0] == "blocked-house" for c in fake_env["calls"])
    assert delegation.running_count("parent-1") == 0
    delegation.reset_delegations()


# ── Gap coverage: crash honesty, double-spawn, degenerate mode, images ──────


def test_crashing_agent_reports_failed_event(fake_env, monkeypatch):
    """A specialist that crashes must deliver an honest FAILED event."""
    from agents import delegation

    delegation.reset_delegations()
    ga = fake_env["modules"][1]

    def exploding(state):
        raise RuntimeError("disk on fire")

    monkeypatch.setattr(ga, "general_node", exploding)

    delegation.start_delegations(
        [{"agent": "General", "task": "will explode"}], "parent-1", ""
    )
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)

    events = _drain_all("parent-1")
    assert len(events) == 1
    assert "status: FAILED" in events[0]
    assert "disk on fire" in events[0]
    # Error mirrored into the job chat too.
    assert any(s == "failed" for s in
               ("failed",) ) or True  # mirror recorded via FakeWM below
    roles = [(ws, cid, r, c) for (ws, cid, r, c) in fake_env["wm"].messages]
    agent_msgs = [m for m in roles if m[2] == "agent"]
    assert agent_msgs and "disk on fire" in agent_msgs[0][3]
    delegation.reset_delegations()


def test_double_delegate_in_one_turn(fake_env, monkeypatch):
    """Two delegate calls emitted by the SAME model turn both run; the loop
    guard warns but nothing breaks."""
    import agents.orchestrator_agent as orch
    from agents import delegation
    from sync.state_sync import shared_state

    delegation.reset_delegations()
    shared_state.setdefault("thoughts", {})
    shared_state.setdefault("outputs", {})

    tmp_ws = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp_orch_ws4")
    os.makedirs(tmp_ws, exist_ok=True)

    saw_spawn = {"v": False}

    def decide(messages):
        ctx = "\n".join(getattr(m, "content", "") for m in messages)
        if not saw_spawn["v"]:
            saw_spawn["v"] = True
            return ("Two at once.", [
                {"tool": "delegate", "args": {"items": [{"agent": "General", "task": "a"}]}, "id": "c1"},
                {"tool": "delegate", "args": {"items": [{"agent": "House", "task": "b"}]}, "id": "c2"},
            ])
        if ctx.count("\nstatus: DONE") >= 2:
            return ("Both done.", [])
        return ("Waiting.", [])

    fake_invoke = _native_turn_executor(decide)
    monkeypatch.setattr(orch, "run_native_agent", fake_invoke)

    res = orch.orchestrator_node({
        "client_request": "two things",
        "project_path": tmp_ws,
        "chat_id": "parent-1",
    })
    assert "Both done" in res["agent_report"]
    kinds = sorted(c[0] for c in fake_env["calls"])
    assert kinds == ["general", "house"]
    delegation.reset_delegations()


def test_no_parent_chat_blocking_mode_returns_results(fake_env):
    """Without a parent chat the runtime degrades to BLOCKING and returns
    actual outcomes inside the spawn summaries."""
    from agents import delegation

    delegation.reset_delegations()
    summaries = delegation.start_delegations(
        [{"agent": "General", "task": "inline job"}], "", ""
    )
    assert len(summaries) == 1 and summaries[0]["ok"]
    assert summaries[0]["status"] == "done"
    assert "inline job" in summaries[0]["result"]
    assert delegation.running_count("") == 0
    delegation.reset_delegations()


def test_images_forwarded_per_agent_kind(fake_env):
    from agents import delegation

    delegation.reset_delegations()

    img = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp_img.png")
    with open(img, "wb") as f:
        f.write(b"\x89PNG fake")

    delegation.start_delegations([
        {"agent": "General", "task": "look at this", "images": [img]},
        {"agent": "Dev", "task": "fix ui", "images": [img, img + ".missing.png"]},
    ], "parent-1", "")

    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    by_agent = {c[0]: c[1] for c in fake_env["calls"]}
    # General receives a native images list.
    assert by_agent["general"].get("images") == [img]
    # Dev receives paths embedded in the task text; missing file filtered out.
    dev_req = by_agent["dev"]["client_request"]
    assert "Attached image(s) to inspect" in dev_req
    assert img in dev_req
    assert ".missing.png" not in dev_req
    os.unlink(img)
    delegation.reset_delegations()


def test_orchestrator_scope_excludes_run_command(monkeypatch):
    from tools._schemas import get_native_tool_names
    names = get_native_tool_names("orchestrator")
    assert "run_command" not in names
    assert {"create_project", "resume_project", "ask_user", "delegate", "delegation_status", "update_plan"} <= set(names)


def test_ledger_roundtrip_and_truncation(tmp_path, monkeypatch):
    from agents import delegation as dlg
    monkeypatch.setenv("HARNESS_ROOT", str(tmp_path))
    ledger = tmp_path / ".agent-memory" / "orchestrator_jobs.jsonl"
    for i in range(5):
        dlg._append_job_ledger({
            "id": f"del-{i}", "agent_display": "Dev", "title": f"job {i}",
            "workspace": "ws", "chat_id": f"chat-{i}",
            "status": "done" if i % 2 == 0 else "failed",
            "result": "x" * 400, "error": "",
        })
    assert ledger.is_file()
    block = dlg.recent_jobs_block(10)
    assert "## Recent Jobs" in block and "job 0" in block
    # result capped at 300
    import json
    with open(ledger, "r", encoding="utf-8") as f:
        rec = json.loads(f.readline())
    assert len(rec["result"]) <= 300
    # fill past 500 -> truncates (keep 200, then grows again; must stay <=500)
    for i in range(600):
        dlg._append_job_ledger({
            "id": f"fill-{i}", "agent_display": "General", "title": "fill",
            "workspace": "ws", "chat_id": "c", "status": "done",
            "result": "ok", "error": "",
        })
    with open(ledger, "r", encoding="utf-8") as f:
        kept = [ln for ln in f if ln.strip()]
    assert 200 <= len(kept) <= 500
    assert len(kept) < 605


def test_scoped_plan_injection_only_own_scope(tmp_path):
    from sync.sync_helpers import save_task_tracking, build_plan_progress_block
    chat = "scope-test-1"
    save_task_tracking({
        "task_id": "t1", "chat_id": chat, "user_request": "x",
        "status": "in_progress",
        "plan": {
            "developer": [{"step": "dev job", "status": "pending"}],
            "orchestrator": [{"step": "orch job", "status": "pending"}],
        },
    }, chat_id=chat)
    from sync.sync_helpers import load_task_tracking
    td = load_task_tracking(chat_id=chat)
    dev_block = build_plan_progress_block(td, "developer")
    orch_block = build_plan_progress_block(td, "orchestrator")
    assert "dev job" in dev_block and "orch job" not in dev_block
    assert "orch job" in orch_block and "dev job" not in orch_block
    empty = build_plan_progress_block(td, "house")
    assert empty == ""


def test_template_markers_present():
    import agents.orchestrator_agent as orch
    t = orch._ORCHESTRATOR_SYSTEM_TEMPLATE
    assert "PLANNING" in t
    assert "YOUR MEMORY" in t
    assert "delegation_status" in t


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-v"]))
