"""Self-test for P6 continuable subagents. Run: venv312\\Scripts\\python.exe scratch/test_subagent_manager.py"""
import os
import sys
import tempfile
import threading
import time
from types import SimpleNamespace

# Unit-test the mocked path directly so the continuable loop never calls a
# provider. The runtime imports the streaming entry point at turn time.
os.environ["TOKEN_STREAMING"] = "0"

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "Desktop"))

PASS = 0


def check(label, cond):
    global PASS
    if not cond:
        raise AssertionError(f"FAIL: {label}")
    PASS += 1
    print(f"  ok {PASS}: {label}")


def _native_final(text: str) -> tuple:
    """Native slice for a final (no-tool) turn: (text, [AIMessage], None)."""
    from langchain_core.messages import AIMessage
    return (text, [AIMessage(content=text)], None)


def main():
    import llm
    from agents import subagent_manager
    from agents.subagent_manager import (start_continuable, send_message, interrupt,
                                  list_agents, report, _ACTIVATIONS, reset_manager,
                                  spawn_continuable, child_status)
    from server.realtime.inbox import pending_counts, clear_inbox_registry

    ws = tempfile.mkdtemp(prefix="cont_")
    reset_manager()

    # Patch the specialist backend directly (A5: it now returns a native
    # transcript slice; the turn loop extends history from it).
    from tools.AddOnTool import subagentTools as sm_mod

    # ── 1. first turn with a mocked "final answer" LLM ──
    sm_mod._specialist_call_backend = lambda name, messages, native_tools=None, schema=None, model=None, temp=0.2, reasoning_effort=None, where="": _native_final("Done with the task.")
    r = start_continuable("chatA", ws, "general-purpose", "Do something", label="research")
    child_id = r["child_id"]
    check("start returns child_id", child_id.startswith("cont-"))
    check("first turn result", "Done with the task." in r["result"])
    check("status idle", r["status"] == "idle")

    # child session log contains descriptor + user + assistant
    from server.session.log import SessionManager
    path = subagent_manager._find_child_session(child_id, ws)
    check("child session on disk", path is not None)
    mgr = SessionManager.open(path)
    types = [e.type for e in mgr.get_entries()]
    check("descriptor entry", any(
        getattr(e, "custom_type", "") == "subagent/descriptor" for e in mgr.get_entries()))
    check("user + assistant recorded", types.count("message") >= 2)
    check("parent chat recorded", subagent_manager._descriptor_of(mgr)["parent_chat_id"] == "chatA")

    # ── 2. durable enumeration ──
    agents = list_agents(ws, chat_id="chatA")
    check("list_agents finds child", len(agents) == 1 and agents[0]["child_id"] == child_id)
    check("list_agents label", agents[0]["label"] == "research")
    check("list_agents status idle", agents[0]["status"] == "idle")
    check("list_agents filters chat", list_agents(ws, chat_id="other") == [])

    # ── 3. follow-up turn (cold resume from log) ──
    sm_mod._specialist_call_backend = lambda name, messages, native_tools=None, schema=None, model=None, temp=0.2, reasoning_effort=None, where="": _native_final("Follow-up answered.")
    r2 = send_message(child_id, "Now also check the docs.", ws)
    check("follow-up result", "Follow-up answered." in r2["result"])
    mgr2 = SessionManager.open(path)
    contents = [str(e.content) for e in mgr2.get_entries() if getattr(e, "type", "") == "message"]
    check("follow-up recorded in log", any("Now also check the docs." in c for c in contents))
    check("two assistant turns in log", sum("answered" in c or "Done with the task" in c for c in contents) >= 2)

    # ── 4. queue while running + interrupt ──
    def _looping_mock(name, messages, native_tools=None, schema=None, model=None, temp=0.2, reasoning_effort=None, where=""):
        time.sleep(0.4)  # keep the turn alive long enough to observe "running"
        from langchain_core.messages import AIMessage
        return ("", [AIMessage(content="", tool_calls=[
            {"name": "list_agents", "args": {}, "id": "loop-call"},
        ])], None)
    sm_mod._specialist_call_backend = _looping_mock
    r3_holder = {}

    def worker():
        r3_holder["r"] = start_continuable("chatB", ws, "general-purpose", "Loop forever")

    t = threading.Thread(target=worker, daemon=True)
    t.start()
    # wait for an activation to go running
    deadline = time.time() + 10
    target = None
    while time.time() < deadline:
        for cid, act in list(_ACTIVATIONS.items()):
            if act["status"] == "running":
                target = cid
                break
        if target:
            break
        time.sleep(0.05)
    check("activation running", target is not None)
    q = send_message(target, "queued while running", ws)
    check("message queued while running", q.get("queued") is True)
    ok = interrupt(target)
    check("interrupt accepted", ok is True)
    t.join(timeout=15)
    check("loop stopped with INTERRUPTED", "INTERRUPTED" in r3_holder.get("r", {}).get("result", ""))
    _ACTIVATIONS.pop(target, None)  # drop the chatB activation only

    # ── 5. report → follow-up to parent chat ──
    clear_inbox_registry()
    token = subagent_manager.current_child.set(child_id)
    res = report("progress update from child", quiet=False)
    subagent_manager.current_child.reset(token)
    check("report ok", "OK" in res)
    check("report queued follow-up", pending_counts("chatA")["follow_up"] == 1)
    token = subagent_manager.current_child.set(child_id)
    res2 = report("quiet note", quiet=True)
    subagent_manager.current_child.reset(token)
    check("quiet report ok", "OK" in res2)
    res3 = report("no context")
    check("report outside turn errors", "Error" in res3)

    # ── 6. tools.py surface ──
    import tools
    tools._load_subagents()
    for n in ("send_message", "list_agents", "interrupt_agent", "report"):
        check(f"registry has {n}", n in tools.TOOL_MAP)
    check("general-purpose spec resolves", "general-purpose" in tools._SUBAGENTS_REGISTRY)

    # ── 7. Codex-style lifecycle: spawn / status / steer / stop ──
    gate = threading.Event()

    def _gated_mock(name, messages, native_tools=None, schema=None, model=None, temp=0.2, reasoning_effort=None, where=""):
        gate.wait(timeout=15)
        return _native_final("Lifecycle done.")

    sm_mod._specialist_call_backend = _gated_mock
    r_spawn = spawn_continuable("chatC", ws, "general-purpose", "Background task", label="bg")
    cid = r_spawn.get("child_id", "")
    check("spawn returns immediately", r_spawn.get("status") == "running" and bool(cid))
    st = child_status(cid, wait_seconds=0)
    check("status reports running", st.get("status") == "running")
    q = send_message(cid, "extra instruction while running", ws)
    check("steer queued while running", q.get("queued") is True)
    gate.set()
    st2 = child_status(cid, wait_seconds=20)
    check("status idle after completion", st2.get("status") == "idle")
    check("result captured", "Lifecycle done." in (st2.get("result") or ""))
    check("queued steer drained (2nd turn ran)", sum(
        1 for e in SessionManager.open(subagent_manager._find_child_session(cid, ws)).get_entries()
        if getattr(e, "type", "") == "message" and "extra instruction" in str(getattr(e, "content", ""))
    ) >= 1)

    gate2 = threading.Event()

    def _gated_mock2(name, messages, native_tools=None, schema=None, model=None, temp=0.2, reasoning_effort=None, where=""):
        gate2.wait(timeout=15)
        return _native_final("never finishes")

    sm_mod._specialist_call_backend = _gated_mock2
    r_spawn2 = spawn_continuable("chatC", ws, "general-purpose", "Long task")
    cid2 = r_spawn2.get("child_id", "")
    check("stop accepted", interrupt(cid2) is True)
    gate2.set()
    st3 = child_status(cid2, wait_seconds=20)
    check("interrupted settles to idle", st3.get("status") == "idle")
    check("interrupted marker in result", "INTERRUPTED" in (st3.get("result") or ""))
    check("unknown child status", child_status("cont-nope").get("status") == "unknown")

    print(f"\nALL {PASS} CHECKS PASSED")


if __name__ == "__main__":
    main()
