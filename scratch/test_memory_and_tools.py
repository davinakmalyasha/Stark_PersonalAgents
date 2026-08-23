"""Tests for the compact project-cache contract and tool refinements.

Run:  venv312/Scripts/python.exe -m pytest scratch/test_memory_and_tools.py -q
"""
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _make_sandbox(project_md=""):
    d = tempfile.mkdtemp()
    mem = os.path.join(d, ".agent-memory")
    os.makedirs(mem, exist_ok=True)
    if project_md:
        with open(os.path.join(mem, "project.md"), "w", encoding="utf-8") as f:
            f.write(project_md)
    return d


def test_project_cache_contains_identity_and_compact_map_only():
    from memory.memory_manager import ensure_project_cache, load_cached_code_map

    d = _make_sandbox(
        project_md="# Project Overview\n## Tech Stack\n- Python\n",
    )
    with open(os.path.join(d, "app.py"), "w", encoding="utf-8") as f:
        f.write("from utils import format_money\n")
    with open(os.path.join(d, "utils.py"), "w", encoding="utf-8") as f:
        f.write("def format_money(value):\n    return value\n")

    ensure_project_cache(d)
    code_map = load_cached_code_map(d)
    assert code_map.startswith("projectMap:")
    assert "app.py" in code_map
    assert "utils.py" in code_map
    assert not os.path.exists(os.path.join(d, ".agent-memory", "features.md"))
    assert not os.path.exists(os.path.join(d, ".agent-memory", "lessons.md"))


def test_project_map_is_bounded():
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    for i in range(100):
        with open(os.path.join(d, f"module_{i}.py"), "w", encoding="utf-8") as f:
            f.write(f"def function_{i}(value):\n    return value\n")
    code_map = RepoMapGenerator(d).generate_map()
    assert len(code_map) <= 6000


def test_project_map_context_uses_full_anchor_then_delta():
    from memory.memory_manager import (
        load_cached_code_map,
        refresh_code_map_context,
    )

    d = _make_sandbox()
    with open(os.path.join(d, "app.py"), "w", encoding="utf-8") as f:
        f.write("from utils import value\nprint(value())\n")
    with open(os.path.join(d, "utils.py"), "w", encoding="utf-8") as f:
        f.write("def value():\n    return 1\n")

    first = refresh_code_map_context(d, include_cached_full=True)
    assert first["kind"] == "full"
    assert first["content"].startswith("projectMap:")

    with open(os.path.join(d, "utils.py"), "a", encoding="utf-8") as f:
        f.write("\n# body-only change\n")
    unchanged = refresh_code_map_context(d)
    assert unchanged["kind"] == "none"

    with open(os.path.join(d, "fresh_module.py"), "w", encoding="utf-8") as f:
        f.write("def fresh():\n    return True\n")
    fresh_after_change = refresh_code_map_context(d, include_cached_full=True)
    assert fresh_after_change["kind"] == "full"
    assert fresh_after_change["content"].startswith("projectMap:")
    # The compact map intentionally lists only high-value connected files; the
    # full refresh still snapshots every supported file in its topology state.
    from memory.repo_map_generator import RepoMapGenerator
    assert "fresh_module.py" in RepoMapGenerator(d).topology_snapshot()

    with open(os.path.join(d, "new_module.py"), "w", encoding="utf-8") as f:
        f.write("def created():\n    return True\n")
    delta = refresh_code_map_context(d)
    assert delta["kind"] == "delta"
    assert delta["content"].startswith("projectMap+:")
    assert "new_module.py" in delta["content"]
    # The canonical file stays byte-stable until the checkpoint; this event is
    # transcript-only and must not create a mutable cumulative map.
    assert "new_module.py" not in load_cached_code_map(d)

    rebuilt = refresh_code_map_context(d, force_full=True)
    assert rebuilt["kind"] == "full"
    assert rebuilt["content"].startswith("projectMap:")
    assert "projectMap+:" not in rebuilt["content"]


def test_checkpoint_replaces_old_project_map_deltas_with_full_anchor():
    from langchain_core.messages import AIMessage, HumanMessage, SystemMessage
    from memory.context_compaction import checkpoint_compact

    full = "<current-context>\nprojectMap:\nKey files:\n- app.py\n</current-context>"
    delta = "<current-context>\nprojectMap+:\n+ new.py -> none\n</current-context>"
    messages = [
        SystemMessage(content="stable"),
        HumanMessage(content=full),
        HumanMessage(content="## Task\nold task"),
        AIMessage(content="old analysis"),
        HumanMessage(content="old tool output"),
        HumanMessage(content=delta),
        HumanMessage(content="## Task\nnew task"),
        AIMessage(content="new analysis"),
        HumanMessage(content="new tool output"),
    ]

    compacted = checkpoint_compact(
        messages,
        tool_call_log=[],
        created=[],
        modified=[],
        keep_last_n=3,
    )
    contents = [str(message.content) for message in compacted]
    assert any("projectMap:" in content for content in contents)
    assert not any("projectMap+:" in content for content in contents)
    assert any("new task" in content for content in contents)


def test_project_cache_never_records_session_text():
    from memory.memory_manager import ensure_project_cache

    d = _make_sandbox()
    with open(os.path.join(d, "main.py"), "w", encoding="utf-8") as f:
        f.write("print('ok')\n")
    ensure_project_cache(d)
    memory_files = os.listdir(os.path.join(d, ".agent-memory"))
    assert "features.md" not in memory_files
    assert "lessons.md" not in memory_files
    assert all("session" not in name.lower() for name in memory_files)


def test_symbol_doc_python_docstring():
    from memory.repo_map_generator import _extract_symbol_doc
    lines = ["def foo(a):", '    """Rounds floats to 2 decimals, handles negative zero."""', "    return a"]
    doc = _extract_symbol_doc(lines, 0, "python")
    assert "Rounds floats" in doc


def test_symbol_doc_js_leading_comment():
    from memory.repo_map_generator import _extract_symbol_doc
    lines = ["// Formats a price with currency", "function formatMoney(v) {", "  return v;", "}"]
    doc = _extract_symbol_doc(lines, 1, "javascript")
    assert "Formats a price" in doc


def test_get_file_signatures_three_tuple():
    from memory.repo_map_generator import get_file_signatures
    d = tempfile.mkdtemp()
    path = os.path.join(d, "utils.py")
    with open(path, "w", encoding="utf-8") as f:
        f.write('def formatMoney(val, decimals):\n    """Rounds a float to N decimals."""\n    return round(val, decimals)\n')
    sigs = get_file_signatures(path, ".py", is_hot=True)
    assert sigs and len(sigs[0]) == 3  # (sig, depth, doc)
    sig, depth, doc = sigs[0]
    assert "formatMoney" in sig
    assert "Rounds a float" in doc


def test_dev_registry_is_loaded():
    from tools import _SUBAGENTS_REGISTRY, _load_subagents
    if not _SUBAGENTS_REGISTRY:
        _load_subagents()
    assert _SUBAGENTS_REGISTRY["Dev"]["name"] == "Dev"


def test_read_file_cached_stub():
    """M1: a repeat full read must return a compact CACHED stub, not the full
    content (re-emitting it would add fresh cache-miss tokens for nothing)."""
    import tools as tools_mod
    d = tempfile.mkdtemp()
    path = os.path.join(d, "utils.js")
    with open(path, "w", encoding="utf-8") as f:
        f.write("function foo() { return 42; }\nmodule.exports = { foo };\n")
    key = os.path.normpath(path).lower()
    content = open(path, encoding="utf-8").read()
    tools_mod._FILE_CACHE[key] = (os.path.getmtime(path), f"[FILE] {path} (lines 1-2 of 2)\n{content}")
    try:
        res = tools_mod.read_file(path)
        assert "CACHED READ" in res, f"expected stub, got: {res[:120]!r}"
        assert "function foo" not in res  # full content NOT re-emitted
    finally:
        tools_mod._FILE_CACHE.pop(key, None)


def test_read_file_cache_lifecycle():
    """Natural flow: first read stores, second read stubs, edit invalidates."""
    import tools as tools_mod
    d = tempfile.mkdtemp()
    path = os.path.join(d, "utils.js")
    with open(path, "w", encoding="utf-8") as f:
        f.write("function foo() { return 42; }\nmodule.exports = { foo };\n")
    key = os.path.normpath(path).lower()
    tools_mod._FILE_CACHE.pop(key, None)
    try:
        first = tools_mod.read_file(path)
        assert "function foo" in first  # first read: full content
        assert key in tools_mod._FILE_CACHE
        second = tools_mod.read_file(path)
        assert "CACHED READ" in second  # second read: stub
        with open(path, "a", encoding="utf-8") as f:
            f.write("// edited\n")  # mtime change invalidates the cache
        third = tools_mod.read_file(path)
        assert "// edited" in third  # edited file: full content again
    finally:
        tools_mod._FILE_CACHE.pop(key, None)


def test_run_command_output_dedup():
    """C: re-running the same command with byte-identical output returns a
    REPEAT stub instead of re-appending the full output (fresh cache-miss text)."""
    import tools as tools_mod
    tools_mod._RUN_OUTPUT_CACHE.clear()
    try:
        first = tools_mod.run_command('node -e "console.log(42)"')
        assert "[OK] Exit 0" in first and "42" in first
        second = tools_mod.run_command('node -e "console.log(42)"')
        assert "REPEAT RUN" in second, f"expected stub, got: {second[:120]!r}"
        assert "42" not in second  # identical output NOT re-appended
        # a different output must NOT be stubbed
        third = tools_mod.run_command('node -e "console.log(43)"')
        assert "REPEAT RUN" not in third and "43" in third
    finally:
        tools_mod._RUN_OUTPUT_CACHE.clear()


if __name__ == "__main__":
    for fn in list(globals()):
        if fn.startswith("test_"):
            globals()[fn]()
            print(f"PASS {fn}")
