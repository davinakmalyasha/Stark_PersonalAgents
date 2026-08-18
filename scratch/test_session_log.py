"""Self-test for session_log.py (P1). Run: venv312\\Scripts\\python.exe scratch/test_session_log.py"""
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from session_log import (
    SessionManager, convert_to_llm, tool_call_block, text_block, thinking_block,
    session_dir_for,
)

PASS = 0


def check(label, cond):
    global PASS
    if not cond:
        raise AssertionError(f"FAIL: {label}")
    PASS += 1
    print(f"  ok {PASS}: {label}")


def main():
    tmp = tempfile.mkdtemp(prefix="sess_log_")
    ws = os.path.join(tmp, "ws")
    os.makedirs(ws)

    # 1. create + header
    mgr = SessionManager.create(ws, session_dir=session_dir_for(ws), name="test-session")
    path = mgr.get_session_file()
    check("session file created", path and os.path.isfile(path))
    check("header cwd", mgr.get_cwd() == ws)
    check("leaf is None on fresh session", mgr.get_leaf_id() is None)

    # 2. append messages
    u1 = mgr.append_message("user", "fix the bug in X")
    a1 = mgr.append_assistant(
        [text_block("Let me delegate."), tool_call_block("task", {"agent": "Dev", "prompt": "fix X"}, "call_1")],
        provider="deepseek", model="deepseek-v4-flash", stop_reason="toolUse",
        usage={"input": 100, "output": 50, "cache_read": 200, "cache_write": 10, "total_tokens": 150, "cost": 0.01},
    )
    tr1 = mgr.append_tool_result("fixed by changing Y", tool_call_id="call_1", tool_name="task")
    a2 = mgr.append_assistant([text_block("Done.")], stop_reason="stop")
    check("parent chain", mgr.get_entry(a1).parent_id == u1 and mgr.get_entry(a2).parent_id == tr1)
    check("leaf = last", mgr.get_leaf_id() == a2)

    # 3. compaction with retainedTail
    tail = [
        {"type": "message", "id": "t1", "role": "user", "content": "latest request"},
        {"type": "message", "id": "t2", "role": "assistant", "content": [{"type": "text", "text": "latest reply"}],
         "stop_reason": "stop"},
    ]
    c1 = mgr.append_compaction(
        "## Goal\nfix X\n## Progress\nDone: changed Y",
        first_kept_entry_id=a2, retained_tail=tail,
        tokens_before=8123, read_files=["a.py"], modified_files=["a.py"],
    )
    u2 = mgr.append_message("user", "also check tests")
    a3 = mgr.append_assistant([text_block("Checked.")], stop_reason="stop")

    # 4. context entries honor checkpoint
    ctx = mgr.build_context_entries()
    types = [e.type for e in ctx]
    check("compaction on path", "compaction" in types)
    check("entries after compaction included", any(e.id == u2 for e in ctx) and any(e.id == a3 for e in ctx))
    check("pre-compaction entries shadowed", not any(e.id in (u1, a1, tr1, a2) for e in ctx))
    comp = next(e for e in ctx if e.type == "compaction")
    check("retainedTail kept", comp.retained_tail is not None and len(comp.retained_tail) == 2)
    check("file tracking details", comp.details and "a.py" in comp.details["modified_files"])

    # 5. convert_to_llm
    msgs = convert_to_llm(ctx)
    roles = [type(m).__name__ for m in msgs]
    check("human summary first", roles[0] == "HumanMessage" and "Context summary" in str(msgs[0].content))
    check("retained tail user", any(getattr(m, "content", "") == "latest request" for m in msgs))
    check("retained tail assistant", any("latest reply" in str(getattr(m, "content", "")) for m in msgs))
    check("after-compaction user", any(getattr(m, "content", "") == "also check tests" for m in msgs))

    # native tool calls conversion on raw entries
    raw_msgs = convert_to_llm(mgr.get_entries())
    ai_with_tools = [m for m in raw_msgs if type(m).__name__ == "AIMessage" and m.tool_calls]
    check("native tool_calls from blocks", len(ai_with_tools) == 1 and ai_with_tools[0].tool_calls[0]["name"] == "task")
    text_msgs = convert_to_llm(mgr.get_entries(), native_tool_calls=False)
    check("text-format tool blocks fallback", any("```tool" in str(m.content) for m in text_msgs))
    no_thinking = [m for m in convert_to_llm(mgr.get_entries(), include_thinking=True) if "thinking" in str(getattr(m, "content", ""))]
    thinking = mgr.append_assistant([thinking_block("secret chain"), text_block("answer")], stop_reason="stop")
    stripped = convert_to_llm(mgr.get_entries())
    check("thinking stripped by default", not any("secret chain" in str(m.content) for m in stripped))
    kept = convert_to_llm(mgr.get_entries(), include_thinking=True)
    check("thinking kept on demand", any("secret chain" in str(m.content) for m in kept))

    # 6. branching: move leaf back and append
    mgr.branch(u1)
    b1 = mgr.append_message("user", "alternate approach")
    b2 = mgr.append_assistant([text_block("Alternate done.")], stop_reason="stop")
    check("branch leaf", mgr.get_leaf_id() == b2)
    tree = mgr.get_tree()
    check("tree has 2 children on u1", len(tree[u1]["children"]) >= 2)
    ctx_b = mgr.build_context_entries()
    check("branch context uses alternate path", any(e.id == b1 for e in ctx_b) and not any(e.id == a2 for e in ctx_b))

    # 7. reopen: leaf = file tail
    mgr2 = SessionManager.open(path)
    check("reopen leaf = tail", mgr2.get_leaf_id() == mgr.get_entries()[-1].id)
    check("reopen entries count", len(mgr2.get_entries()) == len(mgr.get_entries()))
    check("reopen session name", mgr2.get_session_name() == "test-session")

    # 8. custom entries + model change stay in context-entries (UI render) but are
    #    filtered out by convert_to_llm
    mgr.append_custom_entry("my-state", {"count": 42})
    mgr.append_model_change("opencode", "deepseek-v4-flash")
    ctx_c = mgr.build_context_entries()
    check("custom/model_change preserved for UI", any(e.type in ("custom", "model_change") for e in ctx_c))
    llm_c = convert_to_llm(ctx_c)
    produced = [e for e in ctx_c if e.type in ("message", "compaction", "branch_summary")]
    check("custom/model_change filtered from LLM view", len(llm_c) == len(produced))

    # 9. fork
    mgr3 = SessionManager.fork_from(path, ws)
    check("fork copies entries", len(mgr3.get_entries()) == len(mgr.get_entries()))
    check("fork records parent", mgr3.get_header().parent_session == path)

    print(f"\nALL {PASS} CHECKS PASSED")


if __name__ == "__main__":
    main()
