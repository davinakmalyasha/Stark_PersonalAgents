"""Codex-parity compaction tests: structured input, ledger pass, uncapped
final summary, trim-retry overflow fallback, retention, summary-last layout.

Run:  venv312/Scripts/python.exe -m pytest scratch/test_compaction_codex.py -q
"""
import asyncio
import os
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from pydantic_ai.messages import (  # noqa: E402
    ModelRequest,
    ModelResponse,
    SystemPromptPart,
    TextPart,
    ThinkingPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)

_LEDGER_MARK = "LEDGER WRITER"


def _sandbox():
    d = tempfile.mkdtemp()
    os.makedirs(os.path.join(d, ".agent-memory"), exist_ok=True)
    with open(os.path.join(d, "app.py"), "w", encoding="utf-8") as f:
        f.write("def run():\n    return 1\n")
    return d


def _history():
    return [
        ModelRequest(parts=[SystemPromptPart(content="STABLE SYSTEM")]),
        ModelRequest(parts=[UserPromptPart(content="## Task\nfirst ask")]),
        ModelResponse(parts=[TextPart(content="working on it")]),
        ModelResponse(parts=[ToolCallPart(tool_name="read_file", args={"file_path": "app.py"}, tool_call_id="c1")]),
        ModelRequest(parts=[ToolReturnPart(tool_name="read_file", content="TOOL PAYLOAD", tool_call_id="c1")]),
        ModelRequest(parts=[UserPromptPart(content="<current-context>\nprojectMap: v2\n</current-context>")]),
        ModelRequest(parts=[UserPromptPart(content="## Task\nsecond ask")]),
    ]


def _last_user_prompt(messages) -> str:
    for message in reversed(messages):
        for part in reversed(getattr(message, "parts", []) or []):
            if getattr(part, "part_kind", "") == "user-prompt":
                return str(getattr(part, "content", "") or "")
    return ""


def _fake_engine_request(calls, ledger_text=None, final_text="[SUMMARY]\nFINAL BODY\n[PROJECT.MD]\nNO_CHANGE"):
    if ledger_text is None:
        ledger_text = "LEDGER BODY " + ("detail notes " * 20)
    async def fake_request(model, messages, **kwargs):
        prompt = _last_user_prompt(messages)
        calls.append({"prompt": prompt, "messages": list(messages)})
        if "For each source file below" in prompt:
            content = "app.py — Runs things."
        elif _LEDGER_MARK in prompt:
            content = ledger_text
        else:
            content = final_text
        return SimpleNamespace(parts=[SimpleNamespace(part_kind="text", content=content)])
    return fake_request


def test_render_full_history_has_no_caps():
    from llm.model_history import render_full_history

    blob = "x" * 50000
    history = [
        ModelRequest(parts=[SystemPromptPart(content="S" * 5000)]),
        ModelResponse(parts=[TextPart(content=blob)]),
        ModelRequest(parts=[UserPromptPart(content="short ask")]),
    ]
    text = render_full_history(history)
    assert blob in text
    assert "S" * 5000 in text


def test_render_history_blocks_tool_calls_and_media():
    from llm.model_history import render_history_blocks

    history = [
        ModelResponse(parts=[ToolCallPart(tool_name="read_file", args={"file_path": "a.py"}, tool_call_id="c1")]),
        ModelRequest(parts=[ToolReturnPart(tool_name="read_file", content="FULL RESULT", tool_call_id="c1")]),
        ModelResponse(parts=[ThinkingPart(content="reasoning here")]),
    ]
    joined = "\n".join(render_history_blocks(history))
    assert "[TOOL CALL read_file]" in joined
    assert '"file_path": "a.py"' in joined
    assert "[TOOL RESULT read_file]" in joined and "FULL RESULT" in joined
    assert "reasoning here" in joined


def test_allocate_chunks_two_parts_with_ledger():
    from memory.compaction_engine import allocate_chunks, estimate_message_tokens

    # ~400k estimated tokens: 4 messages x 350k chars.
    history = [ModelRequest(parts=[SystemPromptPart(content="sys")])]
    for i in range(4):
        history.append(ModelRequest(parts=[UserPromptPart(content=f"m{i}-" + "z" * 350000)]))
    system, chunks = allocate_chunks(history, 262144)
    assert system is not None
    assert len(chunks) == 2  # 210k intermediate + 200k raw last
    first = sum(estimate_message_tokens(m) for m in chunks[0])
    last = sum(estimate_message_tokens(m) for m in chunks[1])
    assert first <= 210001
    assert last <= 200001


def test_allocate_chunks_clamps_to_small_window():
    from memory.compaction_engine import allocate_chunks, estimate_message_tokens

    history = [ModelRequest(parts=[SystemPromptPart(content="sys")])]
    for i in range(6):
        history.append(ModelRequest(parts=[UserPromptPart(content=f"m{i}-" + "z" * 40000)]))
    _system, chunks = allocate_chunks(history, 30000)
    assert len(chunks) >= 3  # smaller compactor -> more chunks
    assert all(sum(estimate_message_tokens(m) for m in c) <= 30000 for c in chunks)


def test_run_compaction_summary_ledger_flow_mocked():
    from memory.compaction_engine import run_compaction_summary

    d = _sandbox()
    calls = []
    env = {
        "COMPACTION_CONTEXT_WINDOW": "10000",
        "COMPACTION_CHUNK_MAX_TOKENS": "2000",
        "COMPACTION_LAST_CHUNK_MAX_TOKENS": "2000",
        "COMPACTION_LEDGER_MAX_TOKENS": "2048",
        "COMPACTION_RESERVE_TOKENS": "1024",
    }
    # ~5k tokens across messages -> at least one intermediate chunk + raw last.
    history = [ModelRequest(parts=[SystemPromptPart(content="STABLE SYSTEM")])]
    for i in range(5):
        history.append(ModelRequest(parts=[UserPromptPart(content=f"ask {i} " + "a" * 3500)]))
        history.append(ModelResponse(parts=[TextPart(content="work " + "b" * 3500)]))
    history.append(ModelRequest(parts=[UserPromptPart(content="## Task\nlatest ask")]))

    with patch.dict(os.environ, env, clear=False), \
         patch("llm.free_provider.build_free_fallback", return_value=object()), \
         patch("pydantic_ai.direct.model_request", new=AsyncMock(side_effect=_fake_engine_request(calls))):
        result = asyncio.run(run_compaction_summary(history, d))

    assert result["ledgers"] >= 1
    assert result["summary"].startswith("Another language model started")
    assert "FINAL BODY" in result["summary"]
    assert result["user_messages"] and "latest ask" in result["user_messages"][-1]
    assert result["full_map"].startswith("projectMap: v2")
    # ledger pass saw the old chunk, final pass saw the ledger.
    ledger_calls = [c for c in calls if _LEDGER_MARK in c["prompt"]]
    final_calls = [c for c in calls if _LEDGER_MARK not in c["prompt"] and "CURRENT PROJECT.MD" in c["prompt"]]
    assert ledger_calls and final_calls
    assert "LEDGER BODY" in final_calls[-1]["prompt"]
    assert "Budget: up to ~" in ledger_calls[-1]["prompt"]
    # seam overlap: the raw tail of the last old chunk is repeated into pass 2.
    assert result["overlap"] >= 1
    assert result["overlap_tokens"] > 0
    assert "repeat the end of the previous segment" in final_calls[-1]["prompt"]


def test_ledger_never_injected_into_history():
    from memory.compaction_engine import compact_history_with_summary

    calls = []
    env = {
        "COMPACTION_CONTEXT_WINDOW": "10000",
        "COMPACTION_CHUNK_MAX_TOKENS": "2000",
        "COMPACTION_LAST_CHUNK_MAX_TOKENS": "2000",
        "COMPACTION_LEDGER_MAX_TOKENS": "2048",
        "COMPACTION_RESERVE_TOKENS": "1024",
    }
    history = [ModelRequest(parts=[SystemPromptPart(content="STABLE SYSTEM")])]
    for i in range(5):
        history.append(ModelRequest(parts=[UserPromptPart(content=f"ask {i} " + "a" * 3500)]))
        history.append(ModelResponse(parts=[TextPart(content="work " + "b" * 3500)]))
    history.append(ModelRequest(parts=[UserPromptPart(content="## Task\nlatest ask")]))

    with patch.dict(os.environ, env, clear=False), \
         patch("llm.free_provider.build_free_fallback", return_value=object()), \
         patch("pydantic_ai.direct.model_request", new=AsyncMock(side_effect=_fake_engine_request(calls))):
        new_history, info = compact_history_with_summary(history, project_path="")

    assert info["ledgers"] >= 1
    joined = "\n".join(
        str(getattr(p, "content", ""))
        for m in new_history for p in getattr(m, "parts", []) or []
    )
    assert "LEDGER BODY" not in joined  # ledger is scratch, never injected
    assert "FINAL BODY" in joined  # only the summary survives
    assert joined.rstrip().endswith("FINAL BODY")  # summary LAST


def test_final_summary_uncapped():
    from memory.compaction_engine import run_compaction_summary

    calls = []
    long_summary = "S" * 30000

    async def fake_request(model, messages, **kwargs):
        calls.append(_last_user_prompt(messages))
        if _LEDGER_MARK in calls[-1]:
            return SimpleNamespace(parts=[SimpleNamespace(
                part_kind="text", content="ledger notes " * 30,
            )])
        return SimpleNamespace(parts=[SimpleNamespace(
            part_kind="text", content=f"[SUMMARY]\n{long_summary}\n[PROJECT.MD]\nNO_CHANGE",
        )])

    with patch("llm.free_provider.build_free_fallback", return_value=object()), \
         patch("pydantic_ai.direct.model_request", new=AsyncMock(side_effect=fake_request)):
        result = asyncio.run(run_compaction_summary(_history(), ""))

    assert len(long_summary) == 30000
    assert long_summary in result["summary"]  # no [:2000], no max_tokens cap


def test_overflow_trims_oldest_and_retries():
    from memory.compaction_engine import run_compaction_summary

    calls = []

    async def flaky_request(model, messages, **kwargs):
        calls.append(list(messages))
        if len(calls) == 1:
            raise RuntimeError("This model's maximum context length is exceeded")
        return SimpleNamespace(parts=[SimpleNamespace(
            part_kind="text", content="[SUMMARY]\nRECOVERED\n[PROJECT.MD]\nNO_CHANGE",
        )])

    with patch("llm.free_provider.build_free_fallback", return_value=object()), \
         patch("pydantic_ai.direct.model_request", new=AsyncMock(side_effect=flaky_request)):
        result = asyncio.run(run_compaction_summary(_history(), ""))

    assert result["trimmed"] >= 1
    assert "RECOVERED" in result["summary"]
    assert len(calls) == 2
    # system preserved; the oldest conversation item was dropped.
    assert any(getattr(p, "part_kind", "") == "system-prompt"
               for p in calls[1][0].parts)
    first_after_trim = calls[1][1]
    assert first_after_trim.parts[0].content != "## Task\nfirst ask"


def test_retained_user_messages_skip_anchors_and_cap():
    from memory.compaction_engine import collect_retained_user_messages

    history = [
        ModelRequest(parts=[UserPromptPart(content="ask one " + "a" * 200)]),
        ModelRequest(parts=[UserPromptPart(content="<current-context>\nprojectMap\n</current-context>")]),
        ModelRequest(parts=[UserPromptPart(content="<project-memory>\nmemory\n</project-memory>")]),
        ModelRequest(parts=[UserPromptPart(content="Another language model started to solve this problem and produced a summary")]),
        ModelRequest(parts=[UserPromptPart(content="ask two is the newest")]),
    ]
    kept = collect_retained_user_messages(history, max_tokens=10_000)
    assert kept == ["ask one " + "a" * 200, "ask two is the newest"]

    tiny = collect_retained_user_messages(history, max_tokens=5)
    assert len(tiny) == 1
    assert "trimmed" in tiny[0]


def test_compact_history_native_summary_last():
    from llm.model_history import compact_history_native

    out = compact_history_native(
        _history(),
        full_project_map="projectMap: v2\nKey files:",
        project_memory_text="## Identity\n- project: X",
        user_messages=["## Task\nfirst ask", "## Task\nsecond ask"],
        summary_text="HANDOFF PREFIX\nsummary body",
    )
    contents = []
    for message in out:
        for part in getattr(message, "parts", []) or []:
            contents.append(str(getattr(part, "content", "")))
    assert contents[0] == "STABLE SYSTEM"
    assert "<project-memory>" in contents[1]
    assert "<current-context>" in contents[2]
    assert contents[3] == "## Task\nfirst ask"
    assert contents[4] == "## Task\nsecond ask"
    assert contents[-1] == "HANDOFF PREFIX\nsummary body"


def test_build_compactor_pins_laguna_window():
    from memory.compaction_engine import DEFAULT_COMPACTION_MODEL, build_compactor

    with patch("llm.free_provider.build_free_fallback", return_value=object()), \
         patch.dict(os.environ, {"COMPACTION_MODEL": DEFAULT_COMPACTION_MODEL}, clear=False):
        model, window, name = build_compactor()
    assert model is not None
    assert name == DEFAULT_COMPACTION_MODEL
    assert window == 262144


def test_ledger_prompt_structure():
    from memory.compaction_prompts import get_ledger_prompt

    prompt = get_ledger_prompt()
    for section in (
        "LEDGER WRITER",
        "## SEGMENT GOALS",
        "## STATE AT SEGMENT END",
        "## DECISIONS & RATIONALE",
        "## FILES & SYMBOLS",
        "## COMMANDS, TESTS & RESULTS",
        "## ERRORS & FIXES",
        "## CONSTRAINTS & PREFERENCES",
        "## DEAD ENDS",
        "## OPEN THREADS & UNCERTAINTIES",
        "## VERBATIM USER INSTRUCTIONS",
    ):
        assert section in prompt


def test_seam_overlap_keeps_tool_pairs():
    from memory.compaction_engine import _is_tool_return_only, seam_overlap

    chunk = [
        ModelRequest(parts=[UserPromptPart(content="old ask")]),
        ModelResponse(parts=[ToolCallPart(tool_name="read_file", args={"file_path": "a.py"}, tool_call_id="c1")]),
        ModelRequest(parts=[ToolReturnPart(tool_name="read_file", content="RESULT", tool_call_id="c1")]),
    ]
    tail = seam_overlap(chunk, max_tokens=2)
    assert tail  # at least one message
    assert not _is_tool_return_only(tail[0])  # never an orphaned tool return


def test_project_md_no_change_keeps_file_untouched():
    from memory.compaction_engine import run_compaction_summary

    d = _sandbox()
    project_file = os.path.join(d, ".agent-memory", "project.md")
    with open(project_file, "w", encoding="utf-8") as f:
        f.write("## Identity\n- project: Existing\n")
    calls = []

    with patch("llm.free_provider.build_free_fallback", return_value=object()), \
         patch("pydantic_ai.direct.model_request", new=AsyncMock(side_effect=_fake_engine_request(calls))):
        asyncio.run(run_compaction_summary(_history(), d))
    with open(project_file, encoding="utf-8") as f:
        assert "- project: Existing" in f.read()


if __name__ == "__main__":
    for fn in list(globals()):
        if fn.startswith("test_"):
            globals()[fn]()
            print(f"PASS {fn}")
