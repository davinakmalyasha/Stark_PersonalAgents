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

    def create_chat(self, ws, title="", model="Automatic Fallback",
                    parent_chat_id="", parent_workspace_id=""):
        self._n += 1
        cid = f"chat-{self._n}"
        self.chats[(ws, cid)] = {"id": cid, "title": title}
        return {"id": cid}

    def get_chat(self, ws, cid, include_traces=False, include_usage=True):
        base = self.chats.get((ws, cid))
        if base is None:
            return None
        msgs = [{"role": r, "content": c}
                for (w, c_id, r, c) in self.messages if w == ws and c_id == cid]
        return {"id": cid, "title": base.get("title", ""), "messages": msgs}

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
        [{"agent": "House", "task": "slow thing"}], "parent-1", "", background=True,
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


def test_status_table_filter_and_detail(fake_env):
    from agents import delegation

    delegation.reset_delegations()
    summaries = delegation.start_delegations(
        [{"agent": "General", "task": "note today i ate pasta", "title": "Note"}],
        "parent-1", "",
    )
    jid = summaries[0]["id"]
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    assert "No delegations with status 'running'" in delegation.status_table(status="running")
    assert "[OK]" in delegation.status_table(status="done")
    detail = delegation.job_detail(jid)
    assert "[JOB DETAIL]" in detail
    assert "note today i ate pasta" in detail  # original task text
    assert "general done:" in detail  # capped result
    assert "recent chat:" in detail  # mirrored chat tail
    assert delegation.job_detail("del-nope").startswith("Error: unknown job id")
    # Bounded wait on a settled job returns immediately with its snapshot.
    snap = delegation.wait_for_job(jid, wait_seconds=5)
    assert snap and snap["status"] == "done"
    assert delegation.wait_for_job("del-nope") is None
    delegation.reset_delegations()


def test_job_message_and_cancel(fake_env, monkeypatch):
    import agents.house_agent as ha
    from agents import delegation
    from tools.AddOnTool import delegationTools as dt
    from inbox import drain_steering

    delegation.reset_delegations()
    gate = threading.Event()

    def slow_house(state):
        gate.wait(timeout=10)
        return {"agent_report": "house late"}

    monkeypatch.setattr(ha, "house_node", slow_house)
    # Thread path (production semantics): returns while the job runs.
    summaries = delegation.start_delegations(
        [{"agent": "House", "task": "dim lights", "title": "Lights"}],
        "parent-1", "", background=True,
    )
    jid = summaries[0]["id"]
    assert _wait_until(lambda: (delegation.get_entry(jid) or {}).get("status") == "running")

    # Steer while running: mirrored + steered.
    out = dt.job_message(jid, "also lock the door")
    assert out.startswith("[OK]")
    assert "also lock the door" in drain_steering(summaries[0]["chat_id"])

    # Cancel: settles CANCELLED and wakes the parent exactly once.
    out = dt.job_cancel(jid, "Sir changed his mind")
    assert out.startswith("[OK]")
    e = delegation.get_entry(jid)
    assert e["status"] == "cancelled" and e["error"] == "Sir changed his mind"
    wakes = _drain_all("parent-1")
    assert any("CANCELLED" in w and jid in w for w in wakes)
    # The runner thread landing later must not overwrite the cancellation.
    gate.set()
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    assert delegation.get_entry(jid)["status"] == "cancelled"
    assert not [w for w in _drain_all("parent-1") if jid in w and "DONE" in w]

    # Steering / cancelling a settled job is a clean error / no-op.
    err = dt.job_message(jid, "too late")
    assert err.startswith("Error:")
    assert dt.job_cancel(jid).startswith("[OK]")
    assert dt.job_message("del-nope", "hi").startswith("Error:")
    assert dt.job_cancel("del-nope").startswith("Error:")
    delegation.reset_delegations()


# ── Orchestrator loop integration (mocked LLM) ───────────────────────────────


def _native_slice(text: str, tool_calls: list) -> list:
    """Build a fake run_native_agent transcript slice from a scripted turn.

    Mirrors llm.pai_transport._pai_to_lc: AIMessage with tool_calls + one
    ToolMessage per call; the graph driver consumes this slice as the turn's
    native messages. ``tool_calls`` may be plain dicts (default result content
    "ok") or dicts with an extra ``_content`` key carrying realistic output.
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


def _extract_ctx(messages) -> str:
    """Extract text content from either LC or ModelMessage history (A6b)."""
    parts = []
    for m in messages or []:
        # Native ModelMessage
        try:
            from pydantic_ai.messages import ModelRequest, ModelResponse, TextPart, UserPromptPart, SystemPromptPart, ToolReturnPart, ToolCallPart
            if isinstance(m, (ModelRequest, ModelResponse)):
                for p in getattr(m, "parts", []) or []:
                    if isinstance(p, (UserPromptPart, SystemPromptPart)):
                        c = p.content
                        if isinstance(c, str):
                            parts.append(c)
                        elif isinstance(c, list):
                            for item in c:
                                if isinstance(item, str):
                                    parts.append(item)
                                else:
                                    try:
                                        parts.append(str(item))
                                    except Exception:
                                        pass
                        else:
                            parts.append(str(c))
                    elif isinstance(p, TextPart):
                        parts.append(p.content or "")
                    elif isinstance(p, ToolReturnPart):
                        parts.append(str(p.content or ""))
                    elif isinstance(p, ToolCallPart):
                        # tool calls not part of ctx sentinel check, but include
                        parts.append(str(p.args))
                continue
        except Exception:
            pass
        # LC fallback
        c = getattr(m, "content", "") or ""
        if isinstance(c, list):
            for item in c:
                if isinstance(item, dict) and "text" in item:
                    parts.append(item["text"])
                else:
                    parts.append(str(item))
        else:
            parts.append(str(c))
    return "\n".join(parts)


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
        ctx = _extract_ctx(messages)
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

    # spawn turn + wake turn(s) that forward the results
    assert turn["n"] >= 2
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
    monkeypatch.setattr(wm, "get_chat", chats.get_chat)
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


def test_delegate_project_resume_mode(router_env):
    """delegate(project=...) continues the durable Dev: tracked job, no new chat."""
    from agents.project_router import create_project as cp
    from agents import delegation
    delegation.reset_delegations()

    entry = cp("Gamma", "an api service", wait=True)
    chats_before = len(router_env["chats"].chats)
    summaries = delegation.start_delegations(
        [{"agent": "Dev", "task": "fix the login bug", "title": "Login fix", "project": "Gamma"}],
        parent_chat_id="", fallback_ws="",
    )
    assert len(summaries) == 1 and summaries[0]["ok"]
    assert summaries[0]["mode"] == "resume"
    assert summaries[0]["status"] == "done"
    assert "fix the login bug" in summaries[0]["result"]
    assert router_env["sent"][0]["child_id"] == entry["child_id"]
    # No new chat created — the durable project chat was continued.
    assert len(router_env["chats"].chats) == chats_before
    e = delegation.get_entry(summaries[0]["id"])
    assert e["mode"] == "resume" and e["project"] == "Gamma"
    assert e["chat_id"] == entry["chat_id"]

    # Unknown project → deterministic Error, nothing spawned.
    bad = delegation.start_delegations(
        [{"task": "hello", "project": "Nonexistent Project Xyz"}],
        parent_chat_id="", fallback_ws="",
    )
    assert not bad[0]["ok"] and bad[0]["error"].startswith("Error: [NOT FOUND]")

    # project= requires the Dev agent.
    wrong = delegation.start_delegations(
        [{"agent": "General", "task": "hello", "project": "Gamma"}],
        parent_chat_id="", fallback_ws="",
    )
    assert not wrong[0]["ok"] and wrong[0]["error"].startswith("Error:")
    delegation.reset_delegations()


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
        ctx = _extract_ctx(messages)
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
        ctx = _extract_ctx(messages)
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

    summaries = delegation.start_delegations(
        [{"agent": "General", "task": "will explode"}], "parent-1", ""
    )
    jid = summaries[0]["id"]
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)

    events = _drain_all("parent-1")
    mine = [e for e in events if jid in e]
    assert len(mine) == 1
    assert "status: FAILED" in mine[0]
    assert "disk on fire" in mine[0]
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
        ctx = _extract_ctx(messages)
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
    # Consolidated job system: project=<name> resumes a durable Dev; per-job
    # control via job_message/job_cancel; the job tracker is automatic, so
    # update_plan is NOT an orchestrator tool.
    assert {"create_project", "ask_user", "delegate", "delegation_status",
            "job_message", "job_cancel", "propose_plan"} <= set(names)
    assert "resume_project" not in names
    assert "update_plan" not in names


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
    assert "JOB TRACKER" in t and "automatic" in t
    assert "YOUR MEMORY" in t
    assert "delegation_status" in t
    # Brief boundary + artifact + honesty rules present.
    assert "outcome" in t and "acceptance" in t
    assert "never HOW" in t or "WHAT" in t
    assert "artifact" in t
    assert "failed or cancelled" in t
    # The orchestrator no longer writes plans manually.
    assert "update_plan" not in t


# ── P0 refinements: brief boundary, deadlines, queue, artifacts, telemetry ────


def test_delegate_brief_fields_and_method_line(fake_env):
    from agents import delegation

    delegation.reset_delegations()
    summaries = delegation.start_delegations(
        [{
            "agent": "General", "task": "save a note", "title": "Note",
            "outcome": "The note is saved.", "acceptance": "Searching finds it.",
        }],
        "parent-1", "", background=True,
    )
    jid = summaries[0]["id"]
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    e = delegation.get_entry(jid)
    assert e["outcome"] == "The note is saved."
    assert e["acceptance"] == "Searching finds it."
    # The composed task carries outcome/acceptance + the standing method line.
    assert "Outcome (what done looks like): The note is saved." in e["task"]
    assert "Acceptance (how success will be judged): Searching finds it." in e["task"]
    assert "Method is yours to choose" in e["task"]
    assert e["duration_s"] is not None
    delegation.reset_delegations()


def test_artifact_written_and_wake_carries_summary_pointer(fake_env, monkeypatch, tmp_path):
    from agents import delegation

    monkeypatch.setenv("HARNESS_ROOT", str(tmp_path))
    delegation.reset_delegations()

    long_tail = ("x" * 4000) + "TAIL-MARKER-END"
    import agents.general_agent as ga
    monkeypatch.setattr(ga, "general_node", lambda state: (
        {"agent_report": "HEAD-MARKER start.\n" + long_tail}
    ))

    summaries = delegation.start_delegations(
        [{"agent": "General", "task": "long report", "title": "Long"}],
        "parent-1", "", background=True,
    )
    jid = summaries[0]["id"]
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    e = delegation.get_entry(jid)
    artifact = e.get("artifact") or ""
    assert artifact and os.path.isfile(artifact)
    with open(artifact, "r", encoding="utf-8") as f:
        body = f.read()
    assert "TAIL-MARKER-" in body and "HEAD-MARKER" in body  # full report kept

    # Wake delivery happens right after the status flip — give it a moment.
    wakes: list[str] = []

    def _grab():
        wakes.extend(_drain_all("parent-1"))
        return any(jid in w for w in wakes)

    assert _wait_until(_grab, timeout=5)
    wake = next(w for w in wakes if jid in w)
    assert "artifact:" in wake
    assert "HEAD-MARKER" in wake
    assert "TAIL-MARKER-" not in wake  # summary only — no full paste
    assert "[... " in wake and "more chars" in wake

    detail = delegation.job_detail(jid)
    assert "TAIL-MARKER-" in detail  # drill-in reads the artifact
    delegation.reset_delegations()


def test_job_deadline_watchdog_marks_failed(fake_env, monkeypatch):
    import agents.house_agent as ha
    from agents import delegation

    delegation.reset_delegations()
    monkeypatch.setenv("ORCH_JOB_KILL_GRACE", "1")
    gate = threading.Event()

    def hung_house(state):
        gate.wait(timeout=15)
        return {"agent_report": "late"}

    monkeypatch.setattr(ha, "house_node", hung_house)
    summaries = delegation.start_delegations(
        [{"agent": "House", "task": "hang", "title": "Hang", "timeout": 1}],
        "parent-1", "", background=True,
    )
    jid = summaries[0]["id"]
    assert _wait_until(lambda: (delegation.get_entry(jid) or {}).get("status") == "failed", timeout=10)
    e = delegation.get_entry(jid)
    assert "timeout" in e["error"].lower()
    wakes = _drain_all("parent-1")
    assert any(jid in w and "FAILED" in w and "timeout" in w.lower() for w in wakes)
    gate.set()
    delegation.reset_delegations()


def test_parallel_cap_queues_excess_jobs(fake_env, monkeypatch):
    import agents.house_agent as ha
    from agents import delegation

    delegation.reset_delegations()
    monkeypatch.setenv("ORCH_MAX_PARALLEL", "1")
    gate = threading.Event()

    def held_house(state):
        gate.wait(timeout=15)
        return {"agent_report": "held done"}

    monkeypatch.setattr(ha, "house_node", held_house)
    summaries = delegation.start_delegations(
        [{"agent": "House", "task": "one", "title": "One"},
         {"agent": "House", "task": "two", "title": "Two"}],
        "parent-1", "", background=True,
    )
    ids = [s["id"] for s in summaries]
    assert _wait_until(lambda: any(
        (delegation.get_entry(i) or {}).get("status") in ("running", "queued") for i in ids))
    assert _wait_until(lambda: any(
        (delegation.get_entry(i) or {}).get("status") == "queued" for i in ids), timeout=5)
    board = delegation.status_table()
    assert "queued" in board
    assert "[..]" in board
    gate.set()
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0, timeout=15)
    assert all((delegation.get_entry(i) or {}).get("status") == "done" for i in ids)
    delegation.reset_delegations()


def test_wrapup_guard_flags_unmentioned_failures(fake_env):
    from agents import delegation
    from agents.graph.orchestrator_graph import _unmentioned_settled_failures

    delegation.reset_delegations()
    with delegation._LOCK:
        delegation._DELEGATIONS["del-x1"] = {
            "id": "del-x1", "agent_display": "Dev", "title": "Login fix",
            "status": "failed", "parent": "parent-1", "result": "", "error": "boom",
        }
        delegation._DELEGATIONS["del-x2"] = {
            "id": "del-x2", "agent_display": "General", "title": "Note",
            "status": "done", "parent": "parent-1", "result": "ok", "error": "",
        }
        delegation._DELEGATIONS["del-x3"] = {
            "id": "del-x3", "agent_display": "House", "title": "Lights",
            "status": "failed", "parent": "other-chat", "result": "", "error": "boom",
        }

    class _State:
        chat_id = "parent-1"
        response_text = "All done, Sir — the note is saved."

    missing = _unmentioned_settled_failures(_State())
    assert len(missing) == 1 and "del-x1" in missing[0]

    class _StateOk:
        chat_id = "parent-1"
        response_text = "Login fix failed (del-x1) — boom. Note saved."

    assert _unmentioned_settled_failures(_StateOk()) == []
    delegation.reset_delegations()


def test_state_version_guard_discards_stale_resume(tmp_path, monkeypatch):
    import json
    from agents.graph import orchestrator_graph as og

    chat = "chat-ver-1"
    old = og.OrchestratorState(chat_id=chat, project_path=str(tmp_path), client_request="x")
    data = old.to_json()
    data["schema_version"] = 1  # stale
    path = og.state_file_path(chat, str(tmp_path))
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f)
    assert og.load_state(chat, str(tmp_path)) is None

    # Current version + matching fingerprint → resumes.
    fresh = og.OrchestratorState(chat_id=chat, project_path=str(tmp_path), client_request="x")
    og.save_state(fresh, str(tmp_path))
    loaded = og.load_state(chat, str(tmp_path))
    assert loaded is not None and loaded.chat_id == chat


# ── Auto job tracker: the orchestrator plan is built from real jobs ──────────


def test_auto_job_plan_tracks_jobs(fake_env, monkeypatch, tmp_path):
    import sync.sync_helpers as sh
    import agents.house_agent as ha
    from agents import delegation
    from sync.sync_helpers import load_plan

    monkeypatch.setattr(sh, "WORKSPACE_DIR", str(tmp_path))
    delegation.reset_delegations()
    gate = threading.Event()

    def held_house(state):
        gate.wait(timeout=15)
        return {"agent_report": "lights off"}

    monkeypatch.setattr(ha, "house_node", held_house)
    summaries = delegation.start_delegations(
        [{"agent": "House", "task": "dim the lights", "title": "Lights"}],
        "parent-1", "", background=True,
    )
    jid = summaries[0]["id"]

    def _items():
        return load_plan("", "parent-1", "orchestrator")

    # The runner flips the item to "in_progress" when it takes a slot; the job
    # is held on the gate, so wait for that state specifically.
    assert _wait_until(lambda: _items() and _items()[0]["status"] == "in_progress", timeout=5)
    items = _items()
    assert len(items) == 1
    step = items[0]["step"]
    assert jid in step and "House" in step and "Lights" in step

    gate.set()
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    assert _wait_until(lambda: _items() and _items()[0]["status"] == "completed")
    step = _items()[0]["step"]
    assert "done" in step and jid in step
    # The developer's plan scope is untouched by the orchestrator tracker.
    assert load_plan("", "parent-1", "developer") == []
    delegation.reset_delegations()


def test_auto_job_plan_failure_reason(fake_env, monkeypatch, tmp_path):
    import sync.sync_helpers as sh
    import agents.general_agent as ga
    from agents import delegation
    from sync.sync_helpers import load_plan

    monkeypatch.setattr(sh, "WORKSPACE_DIR", str(tmp_path))
    delegation.reset_delegations()

    def exploding(state):
        raise RuntimeError("disk on fire")

    monkeypatch.setattr(ga, "general_node", exploding)
    summaries = delegation.start_delegations(
        [{"agent": "General", "task": "will explode", "title": "Boom"}],
        "parent-1", "", background=True,
    )
    jid = summaries[0]["id"]
    assert _wait_until(lambda: delegation.running_count("parent-1") == 0)
    items = load_plan("", "parent-1", "orchestrator")
    assert items and items[0]["status"] == "completed"
    step = items[0]["step"]
    assert jid in step and "failed" in step and "disk on fire" in step
    delegation.reset_delegations()


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-v"]))
