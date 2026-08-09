"""Tests for memory refinement (code-map descriptions, always-inject, fix-mode,
6K cap) and tool refinements (view_signatures 3-tuples, SDET registry).

Run:  venv312/Scripts/python.exe -m pytest scratch/test_memory_and_tools.py -q
"""
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _make_sandbox(features_md="", project_md=""):
    d = tempfile.mkdtemp()
    mem = os.path.join(d, ".agent-memory")
    os.makedirs(mem, exist_ok=True)
    if project_md:
        with open(os.path.join(mem, "project.md"), "w", encoding="utf-8") as f:
            f.write(project_md)
    if features_md:
        with open(os.path.join(mem, "features.md"), "w", encoding="utf-8") as f:
            f.write(features_md)
    return d


def test_memory_always_includes_code_map():
    from dev_memory_helper import get_developer_memory_context
    from memory_manager import CODE_MAP_START, CODE_MAP_END
    d = _make_sandbox(
        project_md="# Project Overview\n**Auto-generated**: 2026-08-05\n## Tech Stack\n- Node.js\n",
        features_md=(
            "# Features\n"
            f"{CODE_MAP_START}\n"
            "## Code Map (auto-generated)\n"
            "utils.js:\n"
            "\u2502 formatMoney  # Rounds floats to 2 decimals\n"
            f"{CODE_MAP_END}\n"
            "## Cart Feature\n- checkout flow\n"
        ),
    )
    ctx = get_developer_memory_context(d, "fix the cart checkout flow")
    assert "Code Map" in ctx
    assert "formatMoney" in ctx
    assert "Cart Feature" in ctx  # request-matched feature included on top of the map


def test_memory_cap_6k():
    from dev_memory_helper import get_developer_memory_context
    from memory_manager import CODE_MAP_START, CODE_MAP_END
    big = "\n".join(f"line {i} " + "x" * 50 for i in range(300))
    d = _make_sandbox(
        project_md="# Project Overview\n- Node.js\n",
        features_md=f"{CODE_MAP_START}\n## Code Map\n{big}\n{CODE_MAP_END}\n",
    )
    ctx = get_developer_memory_context(d, "anything at all here")
    assert len(ctx) <= 6000


def test_memory_code_map_only_fix_mode():
    from dev_memory_helper import get_developer_memory_context
    from memory_manager import CODE_MAP_START, CODE_MAP_END
    d = _make_sandbox(
        project_md="# Project Overview\n- Node.js\n- something meaningful\n",
        features_md=f"{CODE_MAP_START}\n## Code Map\nutils.js:\n\u2502 formatMoney\n{CODE_MAP_END}\n",
    )
    ctx = get_developer_memory_context(d, "fix", include_code_map_only=True)
    assert "Code Map" in ctx
    assert "formatMoney" in ctx
    assert "Project Overview" not in ctx  # project.md excluded in fix mode


def test_symbol_doc_python_docstring():
    from repo_map_generator import _extract_symbol_doc
    lines = ["def foo(a):", '    """Rounds floats to 2 decimals, handles negative zero."""', "    return a"]
    doc = _extract_symbol_doc(lines, 0, "python")
    assert "Rounds floats" in doc


def test_symbol_doc_js_leading_comment():
    from repo_map_generator import _extract_symbol_doc
    lines = ["// Formats a price with currency", "function formatMoney(v) {", "  return v;", "}"]
    doc = _extract_symbol_doc(lines, 1, "javascript")
    assert "Formats a price" in doc


def test_get_file_signatures_three_tuple():
    from repo_map_generator import get_file_signatures
    d = tempfile.mkdtemp()
    path = os.path.join(d, "utils.py")
    with open(path, "w", encoding="utf-8") as f:
        f.write('def formatMoney(val, decimals):\n    """Rounds a float to N decimals."""\n    return round(val, decimals)\n')
    sigs = get_file_signatures(path, ".py", is_hot=True)
    assert sigs and len(sigs[0]) == 3  # (sig, depth, doc)
    sig, depth, doc = sigs[0]
    assert "formatMoney" in sig
    assert "Rounds a float" in doc


def test_sdet_registry_max():
    from tools import _SUBAGENTS_REGISTRY, _load_subagents
    if not _SUBAGENTS_REGISTRY:
        _load_subagents()
    assert _SUBAGENTS_REGISTRY["SDET"]["reasoning_effort"] == "max"


def test_devsecops_registry_still_max():
    from tools import _SUBAGENTS_REGISTRY, _load_subagents
    if not _SUBAGENTS_REGISTRY:
        _load_subagents()
    assert _SUBAGENTS_REGISTRY["DevSecOps"]["reasoning_effort"] == "max"


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
