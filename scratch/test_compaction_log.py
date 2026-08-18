"""Self-test for P3 session-log compaction. Run: venv312\\Scripts\\python.exe scratch/test_compaction_log.py"""
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

os.environ["BSA_LOG_COMPACT"] = "1"
os.environ["BSA_LOG_LLM_SUMMARY"] = "0"  # deterministic structural fallback

from session_log import SessionManager, session_dir_for, convert_to_llm
from memory.context_compaction import (
    find_log_cut_point, serialize_entries, extract_log_file_ops, compact_session_log,
)

PASS = 0


def check(label, cond):
    global PASS
    if not cond:
        raise AssertionError(f"FAIL: {label}")
    PASS += 1
    print(f"  ok {PASS}: {label}")


def build_log(ws):
    mgr = SessionManager.create(ws, session_dir=session_dir_for(ws))
    for i in range(10):
        mgr.append_message("user", f"request {i} " * 50)
        mgr.append_assistant([{"type": "text", "text": f"answer {i} " * 50}], stop_reason="stop")
        mgr.append_tool_result(f"tool output {i} " * 50, tool_call_id=f"c{i}", tool_name="bash")
    return mgr


def main():
    ws = tempfile.mkdtemp(prefix="comp_")
    mgr = build_log(ws)

    # cut point never lands on a tool result
    entries = mgr.build_context_entries()
    cut, acc = find_log_cut_point(entries, keep_recent_chars=5000)
    check("cut index in range", 0 < cut < len(entries))
    check("cut not at tool result", entries[cut].role in ("user", "assistant"))
    check("accumulated chars", acc >= 5000)

    cut2, _ = find_log_cut_point(entries, keep_recent_chars=10**12)
    check("huge budget -> cut 0", cut2 == 0)

    # serialization framing
    text = serialize_entries(entries[:6])
    check("serialized user lines", "[User]:" in text)
    check("serialized assistant lines", "[Assistant]:" in text)
    check("serialized tool results", "[Tool result]:" in text)

    # file ops extraction from toolCall blocks
    m = SessionManager.create(ws + "_f", session_dir=session_dir_for(ws + "_f"))
    for i in range(4):
        m.append_message("user", f"hi {i}")
        m.append_assistant([{"type": "toolCall", "name": "read_file", "arguments": {"file_path": "a.py"}}], stop_reason="toolUse")
        m.append_tool_result("ok", tool_call_id=f"x{i}a", tool_name="read_file")
        m.append_assistant([{"type": "toolCall", "name": "edit_file", "arguments": {"file_path": "b.py"}}], stop_reason="toolUse")
        m.append_tool_result("ok", tool_call_id=f"x{i}b", tool_name="edit_file")
    r, mod = extract_log_file_ops(m.build_context_entries())
    check("read files tracked", "a.py" in r)
    check("modified files tracked", "b.py" in mod)
    compact_session_log(m, keep_recent_chars=1)  # force checkpoint with file ops
    comp_f = next(e for e in m.build_context_entries() if e.type == "compaction")
    check("file ops in details", comp_f.details and "a.py" in comp_f.details["read_files"] and "b.py" in comp_f.details["modified_files"])

    # compact_session_log writes a checkpoint with retainedTail
    summary = compact_session_log(mgr, keep_recent_chars=5000)
    check("compaction produced summary", summary is not None and len(summary) >= 40)

    entries2 = mgr.build_context_entries()
    comp = next(e for e in entries2 if e.type == "compaction")
    check("checkpoint retainedTail present", comp.retained_tail and len(comp.retained_tail) > 0)
    check("checkpoint firstKeptEntryId", comp.first_kept_entry_id == comp.retained_tail[0].id)
    check("tokens_before recorded", comp.tokens_before > 0)

    # context after compaction: summary + retained tail only
    llm = convert_to_llm(entries2)
    check("summary first in context", "Context summary" in str(llm[0].content))
    check("shadowed entries gone", not any("request 0" in str(getattr(m, "content", "")) for m in llm))

    # reopen: checkpoint still applies
    m2 = SessionManager.open(mgr.get_session_file())
    entries3 = m2.build_context_entries()
    comp2 = next(e for e in entries3 if e.type == "compaction")
    check("checkpoint survives reopen", comp2.retained_tail is not None)

    print(f"\nALL {PASS} CHECKS PASSED")


if __name__ == "__main__":
    main()
