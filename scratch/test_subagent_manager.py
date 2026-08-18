"""Self-test for P6 continuable subagents. Run: venv312\\Scripts\\python.exe scratch/test_subagent_manager.py"""
import os
import sys
import tempfile
import threading
import time
from types import SimpleNamespace

# Unit-test the mocked path: disable token streaming so the continuable
# loop calls the mocked invoke_messages_with_fallback (no real API calls).
os.environ["TOKEN_STREAMING"] = "0"

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

PASS = 0


def check(label, cond):
    global PASS
    if not cond:
        raise AssertionError(f"FAIL: {label}")
    PASS += 1
    print(f"  ok {PASS}: {label}")


def main():
    import llm
    from agents from agents import subagent_manager
    from agents.subagent_manager import (start_continuable, send_message, interrupt,
                                  list_agents, report, _ACTIVATIONS, reset_manager)
    from inbox import pending_counts, clear_inbox_registry

    ws = tempfile.mkdtemp(prefix="cont_")
    reset_manager()

    # ── 1. first turn with a mocked "final answer" LLM ──
    llm.invoke_messages_with_fallback = lambda role, messages, **kw: SimpleNamespace(
        content="Done with the task.", reasoning_content=None,
    )
    r = start_continuable("chatA", ws, "general-purpose", "Do something", label="research")
    child_id = r["child_id"]
    check("start returns child_id", child_id.startswith("cont-"))
    check("first turn result", "Done with the task." in r["result"])
    check("status idle", r["status"] == "idle")

    # child session log contains descriptor + user + assistant
    import session_log
    from session_log import SessionManager
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
    llm.invoke_messages_with_fallback = lambda role, messages, **kw: SimpleNamespace(
        content="Follow-up answered.", reasoning_content=None,
    )
    r2 = send_message(child_id, "Now also check the docs.", ws)
    check("follow-up result", "Follow-up answered." in r2["result"])
    mgr2 = SessionManager.open(path)
    contents = [str(e.content) for e in mgr2.get_entries() if getattr(e, "type", "") == "message"]
    check("follow-up recorded in log", any("Now also check the docs." in c for c in contents))
    check("two assistant turns in log", sum("answered" in c or "Done with the task" in c for c in contents) >= 2)

    # ── 4. queue while running + interrupt ──
    def _looping_mock(role, messages, **kw):
        time.sleep(0.4)  # keep the turn alive long enough to observe "running"
        return SimpleNamespace(
            content='```tool\n{"tool": "list_files", "args": {"path": "/workspace"}}\n```',
        )
    llm.invoke_messages_with_fallback = _looping_mock
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
    tools.ensure_tool_registry()
    for n in ("send_message", "list_agents", "interrupt_agent", "report"):
        check(f"registry has {n}", tools.ensure_tool_registry().get(n) is not None)
    from tools_pkg import get_native_tools
    native_names = {t["function"]["name"] for t in get_native_tools()}
    check("native schemas include P6 tools", {"send_message", "list_agents", "interrupt_agent", "report"} <= native_names)
    tools._load_subagents()
    gp = tools._SUBAGENTS_REGISTRY["general-purpose"]["tools"]
    check("general-purpose has control tools",
          {"send_message", "list_agents", "interrupt_agent", "report"} <= set(gp))

    print(f"\nALL {PASS} CHECKS PASSED")


if __name__ == "__main__":
    main()
