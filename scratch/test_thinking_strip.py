"""Tests for thinking-block stripping (llm.strip_thinking_from_messages) and
subagent handoff compaction (tools._compact_handoff).

Run:  venv312/Scripts/python.exe -m pytest scratch/test_thinking_strip.py -q
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from langchain_core.messages import AIMessage, HumanMessage, SystemMessage


def test_strip_removes_reasoning_content():
    from llm import strip_thinking_from_messages
    msgs = [
        SystemMessage(content="sys"),
        HumanMessage(content="task"),
        AIMessage(content="done", additional_kwargs={"reasoning_content": "deep thoughts", "tool_calls": [{"id": "x", "type": "function", "function": {"name": "f", "arguments": "{}"}}]}),
    ]
    out = strip_thinking_from_messages(msgs, strip=True)
    assert "reasoning_content" not in out[2].additional_kwargs
    assert "tool_calls" in out[2].additional_kwargs  # tool calls preserved
    assert out[0] is msgs[0] and out[1] is msgs[1]  # non-AI messages untouched


def test_strip_disabled_keeps_reasoning():
    from llm import strip_thinking_from_messages
    msgs = [AIMessage(content="done", additional_kwargs={"reasoning_content": "thoughts"})]
    out = strip_thinking_from_messages(msgs, strip=False)
    assert out[0].additional_kwargs["reasoning_content"] == "thoughts"


def test_strip_no_reasoning_untouched():
    from llm import strip_thinking_from_messages
    msgs = [AIMessage(content="plain", additional_kwargs={})]
    out = strip_thinking_from_messages(msgs, strip=True)
    assert out[0] is msgs[0]


def test_invoke_fallback_applies_knob_default():
    from langchain_core.messages import AIMessage, SystemMessage, HumanMessage
    import llm
    os.environ.pop("BSA_STRIP_THINKING", None)
    msgs = [
        SystemMessage(content="sys"),
        HumanMessage(content="task"),
        AIMessage(content="t", additional_kwargs={"reasoning_content": "R"}),
    ]
    out = llm.strip_thinking_from_messages(msgs, strip=True)
    assert all("reasoning_content" not in m.additional_kwargs for m in out if type(m).__name__ == "AIMessage")


def test_compact_handoff_report_passthrough():
    from tools import _compact_handoff
    report = """### AGENT HANDOFF REPORT [FAILED]
- **Agent:** SDET | **Status:** FAILED

#### 1. ERROR CLASSIFICATION
- **Error Category:** `CODE_BUG` (Runtime Logic Error)
- **Primary Exception:** `KeyError: 'jwt_secret'` in `auth_service.py:114`
- **Failing Command:** `pytest tests/test_auth_flow.py::test_token_refresh`

#### 2. FAILURE LOCUS & CONTEXT
- **File:** `src/auth/auth_service.py` (Line 114)
- **Function:** `verify_token()`
- **Trigger:** Expired session payload returns missing key instead of raising TokenExpiredError.

#### 3. MINIMAL ERROR TRAIL (Tail 5 Lines)
```text
  File "src/auth/auth_service.py", line 114, in verify_token
    secret = config['jwt_secret']
KeyError: 'jwt_secret'
```"""
    out = _compact_handoff("Some preamble before the report.\n\n" + report)
    assert out.startswith("### AGENT HANDOFF REPORT [FAILED]")
    assert "KeyError" in out
    assert len(out) <= 4000


def test_compact_handoff_fallback_within_cap():
    from tools import _compact_handoff
    blob = ("I fixed the bug in utils.js and server.js.\n"
            + "Traceback (most recent call last):\n"
            + '  File "utils.js", line 14, in formatMoney\n'
            + "TypeError: val.toFixed is not a function\n"
            + ("noise " * 3000))
    out = _compact_handoff(blob, "SDET")
    assert len(out) <= 4000
    assert out.startswith("### AGENT HANDOFF REPORT [FAILED]")
    assert "TypeError" in out
    assert "utils.js" in out or "server.js" in out
    assert "MINIMAL ERROR TRAIL" in out


def test_compact_handoff_empty():
    from tools import _compact_handoff
    out = _compact_handoff("", "Dev")
    assert out.startswith("### AGENT HANDOFF REPORT [DONE]")


def test_compact_handoff_done_report():
    from tools import _compact_handoff
    out = _compact_handoff("Tests pass. Implemented utils.js and tests/utils.test.js per spec.", "Dev")
    assert out.startswith("### AGENT HANDOFF REPORT [DONE]")
    assert "utils.js" in out
    assert "OUTCOME" in out


def test_compact_handoff_vfs_path_line():
    from tools import _compact_handoff
    out = _compact_handoff("FAILED: crash", "SDET", "/scratch/handoff_log_SDET_x.txt")
    assert "VFS Log Path" in out
    assert "handoff_log_SDET_x.txt" in out


def test_handoff_cap_1000_chars():
    """The handoff report must cap at 1000 chars (was ~4000)."""
    from tools import _compact_handoff, _HANDOFF_MAX_CHARS
    assert _HANDOFF_MAX_CHARS == 1000
    long_blob = ("def buggy():\n" + "    x = 1\n" * 400 +
                 "    return x\n" + "FAILED: ValueError at utils.py:12\n" * 30)
    out = _compact_handoff(long_blob, "Dev")
    assert len(out) <= 1000
    assert out.startswith("### AGENT HANDOFF REPORT")


def test_handoff_passthrough_cap_1000_chars():
    """A model report in the right format is still capped at 1000 chars."""
    from tools import _compact_handoff
    report = ("### AGENT HANDOFF REPORT [DONE]\n- **Agent:** Dev | **Status:** DONE\n"
              "- **Files:** `utils.js`\n\n#### 1. OUTCOME\n- " + ("fixed the rounding issue " * 200))
    out = _compact_handoff(report, "Dev")
    assert len(out) <= 1000


if __name__ == "__main__":
    for fn in list(globals()):
        if fn.startswith("test_"):
            globals()[fn]()
            print(f"PASS {fn}")
