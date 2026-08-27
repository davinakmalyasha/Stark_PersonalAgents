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


def test_project_map_is_deterministic():
    """Same repository state → byte-identical map (cache-safety contract)."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    with open(os.path.join(d, "app.py"), "w", encoding="utf-8") as f:
        f.write("from helpers.utils import u\nfrom core.engine import Engine\nprint(u(), Engine)\n")
    os.makedirs(os.path.join(d, "helpers"), exist_ok=True)
    with open(os.path.join(d, "helpers", "utils.py"), "w", encoding="utf-8") as f:
        f.write("def u():\n    return 1\n")
    os.makedirs(os.path.join(d, "core"), exist_ok=True)
    with open(os.path.join(d, "core", "engine.py"), "w", encoding="utf-8") as f:
        f.write("from helpers.utils import u\n\nclass Engine:\n    def run(self):\n        return u()\n")

    first = RepoMapGenerator(d).generate_map()
    second = RepoMapGenerator(d).generate_map()
    assert first == second
    assert first.startswith("projectMap:")


def test_pagerank_dempotes_utility_files():
    """A near-universally imported utils file must rank BELOW a logic hub."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    os.makedirs(os.path.join(d, "helpers"), exist_ok=True)
    with open(os.path.join(d, "helpers", "utils.py"), "w", encoding="utf-8") as f:
        f.write("def u():\n    return 1\n")
    os.makedirs(os.path.join(d, "core"), exist_ok=True)
    with open(os.path.join(d, "core", "engine.py"), "w", encoding="utf-8") as f:
        f.write("from helpers.utils import u\n\nclass Engine:\n    def run(self):\n        return u()\n")
    # m1..m5 import BOTH files; m6 imports only utils → raw PageRank(utils)
    # exceeds PageRank(engine); the utility penalty must flip the order.
    for i in range(1, 7):
        imports = "from helpers.utils import u\n"
        if i <= 5:
            imports += "from core.engine import Engine\n"
        with open(os.path.join(d, f"m{i}.py"), "w", encoding="utf-8") as f:
            f.write(imports + f"def use{i}():\n    return u()\n")

    gen = RepoMapGenerator(d)
    graph = gen._graph(gen._files())
    scores = gen._pagerank_scores(gen._files(), graph)
    assert scores["helpers/utils.py"] < scores["core/engine.py"], scores

    # Type barrels under a generic directory (src/types/index.ts) are the
    # classic "imported everywhere" utility — caught via path segment.
    from memory.repo_map_generator import _is_utility_file
    assert _is_utility_file("frontend/src/types/index.ts")
    assert not _is_utility_file("app.py")
    assert not _is_utility_file("memory/chat_context.py")

    code_map = gen.generate_map()
    key_section = code_map.split("Key files:", 1)[1].split("Connections:", 1)[0]
    assert "- core/engine.py" in key_section
    assert "- helpers/utils.py" in key_section
    assert key_section.index("- core/engine.py") < key_section.index("- helpers/utils.py")


def test_key_files_cap_generic_utilities():
    """However extreme the fan-in, utilities hold at most
    MAP_MAX_UTILITY_FILES (default 3) of the Key-files slots."""
    from memory.repo_map_generator import RepoMapGenerator, _is_utility_file

    d = _make_sandbox()
    util_names = ["logger.py", "config.py", "types.py", "constants.py", "utils.py"]
    for i, nm in enumerate(util_names):
        with open(os.path.join(d, nm), "w", encoding="utf-8") as f:
            f.write(f"def u{i}():\n    return {i}\n")
    with open(os.path.join(d, "core_engine.py"), "w", encoding="utf-8") as f:
        f.write("class Engine:\n    def run(self):\n        return 1\n")
    for m in range(6):
        imports = "\n".join(f"import {nm[:-3]}" for nm in util_names)
        with open(os.path.join(d, f"f{m}.py"), "w", encoding="utf-8") as f:
            f.write(imports + "\nimport core_engine\n")

    gen = RepoMapGenerator(d)
    code_map = gen.generate_map()
    rest = code_map.split("Key files:\n", 1)[1]
    listed = []
    for line in rest.splitlines():
        if not line.startswith("- "):
            break  # next section reached
        listed.append(line[2:].partition(" — ")[0].strip())
    # 5 generic helpers exist but at most 3 may occupy Key-files slots; every
    # non-utility file (engine + 6 features) must be listed.
    utility_count = sum(1 for p in listed if _is_utility_file(p))
    assert utility_count <= 3, listed
    assert len(listed) == 7 + utility_count, listed
    assert "core_engine.py" in listed


def test_cochange_pairs_from_git_history():
    """Files repeatedly committed together appear as undirected `<->` hints."""
    import subprocess

    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    def w(rel, text):
        p = os.path.join(d, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "a", encoding="utf-8") as f:
            f.write(text)

    def git(*args):
        subprocess.run(["git", *args], cwd=d, capture_output=True, text=True, check=False)

    w("core_models.py", "class M:\n    pass\n")
    w("api_types.ts", "export type M = { a: number };\n")
    git("init")
    git("config", "user.email", "t@t.local")
    git("config", "user.name", "t")
    for i in range(3):
        w("core_models.py", f"# c{i}\n")
        w("api_types.ts", f"// c{i}\n")
        git("add", "-A")
        git("commit", "-qm", f"c{i}")
    # A lone third file changed once must NOT produce a co-change pair.
    w("lonely.py", "x = 1\n")
    git("add", "-A")
    git("commit", "-qm", "solo")

    code_map = RepoMapGenerator(d).generate_map()
    assert "Co-change hints:" in code_map
    # Root-level pair: implicit root scope, no header, tight grammar.
    assert "api_types.ts<->core_models.py" in code_map


def test_cochange_silent_fallback_without_git():
    """No .git / failing git → purely import-derived map, no hints section."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    with open(os.path.join(d, "app.py"), "w", encoding="utf-8") as f:
        f.write("import helper\nprint(helper)\n")
    with open(os.path.join(d, "helper.py"), "w", encoding="utf-8") as f:
        f.write("value = 1\n")

    code_map = RepoMapGenerator(d).generate_map()
    assert code_map.startswith("projectMap:")
    assert "Co-change hints:" not in code_map
    assert "app.py>helper.py" in code_map


def test_connections_scoped_groups():
    """Edges group under `<dir>/` headers: scope-local names are bare,
    cross-scope targets use root-relative refs, Key-file paths stay
    literal for direct tool calls."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    os.makedirs(os.path.join(d, "pkg", "core"), exist_ok=True)
    with open(os.path.join(d, "pkg", "core", "engine.py"), "w", encoding="utf-8") as f:
        f.write("class Engine:\n    def run(self):\n        return 1\n")
    with open(os.path.join(d, "pkg", "core", "bus.py"), "w", encoding="utf-8") as f:
        f.write("from pkg.core.engine import Engine\n\nclass Bus:\n    engine = Engine()\n")
    for i in range(4):
        with open(os.path.join(d, f"svc{i}.py"), "w", encoding="utf-8") as f:
            f.write(
                f"from pkg.core.engine import Engine\nfrom pkg.core.bus import Bus\n\n"
                f"def job{i}():\n    return Bus.engine.run() + {i}\n"
            )

    code_map = RepoMapGenerator(d).generate_map()
    conn = code_map.split("Connections:\n", 1)[1]

    assert "\npkg/core/\n" in code_map  # scope header emitted once
    assert "bus.py>engine.py" in conn  # scope-local edge: bare names
    # Cross-scope refs keep the smallest unambiguous ROOT-relative tail,
    # never fewer than two segments.
    assert "svc0.py>core/engine.py,core/bus.py" in conn

    keys = code_map.split("Key files:\n", 1)[1].split("Connections:", 1)[0]
    assert "pkg/core/engine.py" in keys  # literal, tool-call ready


def test_cross_scope_refs_stay_unambiguous():
    """Two helper.py in different dirs must get distinct minimized refs,
    while same-scope usage stays bare."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    for sub in ("alpha", "beta"):
        os.makedirs(os.path.join(d, sub), exist_ok=True)
        with open(os.path.join(d, sub, "helper.py"), "w", encoding="utf-8") as f:
            f.write("value = 1\n")
        with open(os.path.join(d, sub, "local.py"), "w", encoding="utf-8") as f:
            f.write(f"from {sub}.helper import value\nprint(value)\n")
    with open(os.path.join(d, "app_a.py"), "w", encoding="utf-8") as f:
        f.write("import alpha.helper\n")
    with open(os.path.join(d, "app_b.py"), "w", encoding="utf-8") as f:
        f.write("import beta.helper\n")

    code_map = RepoMapGenerator(d).generate_map()
    assert "alpha/" in code_map and "beta/" in code_map
    # Same-scope references stay bare even though the basename collides.
    assert "local.py>helper.py" in code_map
    # Cross-scope refs grow just enough to stay unambiguous.
    assert "app_a.py>alpha/helper.py" in code_map
    assert "app_b.py>beta/helper.py" in code_map


def test_flat_project_uses_bare_edges():
    """Root-only layouts: no scope headers, bare `src>target` edges, and an
    Areas line without module counts."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    with open(os.path.join(d, "app.py"), "w", encoding="utf-8") as f:
        f.write("import helper\nprint(helper)\n")
    with open(os.path.join(d, "helper.py"), "w", encoding="utf-8") as f:
        f.write("value = 1\n")

    code_map = RepoMapGenerator(d).generate_map()
    lines = code_map.splitlines()
    assert "app.py>helper.py" in code_map
    assert not any(ln.endswith("/") for ln in lines)  # zero scope headers
    areas_line = next(ln for ln in lines if ln.startswith("Areas:"))
    assert "(" not in areas_line  # counts dropped


def test_root_marker_means_repo_root():
    """`./x` resolves to a root-level file referenced from another scope; a
    deep file keeps at least one real directory segment."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    os.makedirs(os.path.join(d, "pkg", "core"), exist_ok=True)
    with open(os.path.join(d, "pkg", "core", "engine.py"), "w", encoding="utf-8") as f:
        f.write("class Engine:\n    pass\n")
    os.makedirs(os.path.join(d, "tools"), exist_ok=True)
    with open(os.path.join(d, "tools", "lib.py"), "w", encoding="utf-8") as f:
        f.write("tool = 1\n")
    with open(os.path.join(d, "helper_root.py"), "w", encoding="utf-8") as f:
        f.write("value = 1\n")
    # Consumer lives in pkg/core: root files and tools/ are cross-scope.
    with open(os.path.join(d, "pkg", "core", "svc.py"), "w", encoding="utf-8") as f:
        f.write(
            "from pkg.core.engine import Engine\n"
            "import helper_root\nimport tools.lib\n"
        )

    code_map = RepoMapGenerator(d).generate_map()
    conn = code_map.split("Connections:\n", 1)[1]
    line = next(ln for ln in conn.splitlines() if ln.startswith("svc.py>"))
    refs = line.split(">", 1)[1].split(",")
    assert refs[0] == "engine.py"  # scope-local hub leads, bare name
    assert "./helper_root.py" in refs  # true root file carries the ./ marker
    assert "tools/lib.py" in refs  # deep file keeps its directory segment


def test_init_and_nested_index_zeroed():
    """__init__.py and nested index.* barrels never take Key slots from
    logic; a ROOT-level index.* survives as a potential entry point."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    os.makedirs(os.path.join(d, "core"), exist_ok=True)
    with open(os.path.join(d, "core", "logic.py"), "w", encoding="utf-8") as f:
        f.write("def go():\n    return 1\n")
    for i in range(1, 8):
        with open(os.path.join(d, f"u{i}.py"), "w", encoding="utf-8") as f:
            f.write(f"from core.logic import go\n\ndef run{i}():\n    return go()\n")
    os.makedirs(os.path.join(d, "pkg"), exist_ok=True)
    with open(os.path.join(d, "pkg", "__init__.py"), "w", encoding="utf-8") as f:
        f.write("MARKER = 1\n")
    for i in range(1, 4):
        with open(os.path.join(d, f"p{i}.py"), "w", encoding="utf-8") as f:
            f.write(f"from pkg import MARKER\nprint(MARKER)\n")
    os.makedirs(os.path.join(d, "feat"), exist_ok=True)
    with open(os.path.join(d, "feat", "index.ts"), "w", encoding="utf-8") as f:
        f.write("export const f = () => 1;\n")
    with open(os.path.join(d, "app.ts"), "w", encoding="utf-8") as f:
        f.write("import { f } from './feat/index';\nconsole.log(f);\n")
    with open(os.path.join(d, "index.js"), "w", encoding="utf-8") as f:
        f.write("module.exports = { boot: true };\n")
    with open(os.path.join(d, "boot.js"), "w", encoding="utf-8") as f:
        f.write("const m = require('./index.js');\nconsole.log(m);\n")

    code_map = RepoMapGenerator(d).generate_map()
    keys = code_map.split("Key files:\n", 1)[1].split("\n\n", 1)[0]
    keys = keys.split("Connections:", 1)[0]
    assert "core/logic.py" in keys
    assert "__init__.py" not in keys  # plumbing, however imported
    assert "feat/index.ts" not in keys  # nested barrel
    assert "index.js" in keys  # ROOT-level entry candidate survives


def test_junk_dirs_ignored():
    """Universal build/temp/log dirs never reach the map."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    for junk in ("logs", "tmp", "coverage", "target", ".cache"):
        os.makedirs(os.path.join(d, junk), exist_ok=True)
        with open(os.path.join(d, junk, "junk_module.py"), "w", encoding="utf-8") as f:
            f.write("value = 1\n")
    os.makedirs(os.path.join(d, "src"), exist_ok=True)
    with open(os.path.join(d, "src", "main.py"), "w", encoding="utf-8") as f:
        f.write("from src.util import v\nprint(v)\n")
    with open(os.path.join(d, "src", "util.py"), "w", encoding="utf-8") as f:
        f.write("v = 1\n")

    code_map = RepoMapGenerator(d).generate_map()
    for junk in ("logs/", "tmp/", "coverage/", "target/", ".cache/"):
        assert junk not in code_map
    # Scoped rendering: src/ header + bare local edge.
    assert "src/" in code_map
    assert "main.py>util.py" in code_map


def test_areas_ranked_by_connected_mass():
    """A large directory of disconnected scripts cannot buy an Areas slot;
    small directories with real hubs outrank it."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    # Seven small areas: hub + leaf that imports it (fits the top-8 cut).
    for i in range(1, 8):
        area = f"a{i}"
        os.makedirs(os.path.join(d, area), exist_ok=True)
        with open(os.path.join(d, area, "hub.py"), "w", encoding="utf-8") as f:
            f.write(f"def h{i}():\n    return {i}\n")
        with open(os.path.join(d, area, "leaf.py"), "w", encoding="utf-8") as f:
            f.write(f"from {area}.hub import h{i}\nprint(h{i}())\n")
    # One big isolated dump: 15 files, zero connections.
    os.makedirs(os.path.join(d, "zzz_bulk"), exist_ok=True)
    for i in range(15):
        with open(os.path.join(d, "zzz_bulk", f"iso{i}.py"), "w", encoding="utf-8") as f:
            f.write(f"value{i} = {i}\n")

    code_map = RepoMapGenerator(d).generate_map()
    areas_line = next(ln for ln in code_map.splitlines() if ln.startswith("Areas:"))
    assert "zzz_bulk" not in areas_line
    assert "a1" in areas_line and "a7" in areas_line


def test_truncation_reports_omitted_count():
    """Hitting the char budget emits an explicit omitted-lines marker."""
    from memory.repo_map_generator import RepoMapGenerator

    d = _make_sandbox()
    prev_env = os.environ.get("MAP_MAX_CHARS")
    os.environ["MAP_MAX_CHARS"] = "2000"
    try:
        # Long file names push the ranked connection stream past the
        # 2000-char budget so truncation must fire.
        base = "component_with_verbose_name"
        with open(os.path.join(d, f"{base}_00.py"), "w", encoding="utf-8") as f:
            f.write("chain = 1\n")
        for i in range(1, 40):
            name = f"{base}_{i:02d}"
            prev_name = f"{base}_{i - 1:02d}"
            with open(os.path.join(d, f"{name}.py"), "w", encoding="utf-8") as f:
                f.write(f"from {prev_name} import chain\nchain()\n")
        code_map = RepoMapGenerator(d).generate_map()
    finally:
        if prev_env is None:
            os.environ.pop("MAP_MAX_CHARS", None)
        else:
            os.environ["MAP_MAX_CHARS"] = prev_env

    assert "[truncated:" in code_map
    assert "lines omitted" in code_map


def test_identity_projection_freezes_at_head():
    """Drift appends ([updated:] / [identity stabilized:]) must never change
    the injected identity bytes; the stability gate still reads the full file."""
    from memory.memory_manager import (
        _card_last_status,
        _format_compact_project_identity,
    )

    head = (
        "# Project\nname: MyApp\ndescription: Demo\npurpose: Build things\n"
        "stack: Python\nversion: unpinned\nstatus: draft\n"
    )
    drifted = (
        head
        + "\n[identity stabilized: 2026-08-26 10:00 UTC]\nstatus: stable\n"
        + "\n[updated: 2026-09-01 10:00 UTC]\nstack: Python, FastAPI, Redis\n"
    )

    before = _format_compact_project_identity(head, "")
    after = _format_compact_project_identity(drifted, "")
    assert before == after
    assert "- stack: Python\n" in before
    assert "FastAPI" not in after
    assert _card_last_status(drifted) == "stable"


def test_system_prompt_bytes_stable_across_card_drift():
    """Regression: card drift appends between turns must not change a single
    SystemMessage byte (today that wipes the entire cached prefix)."""
    from agents.developer_agent import _build_system_prompt

    d = _make_sandbox()
    card_path = os.path.join(d, ".agent-memory", "project.md")
    with open(card_path, "w", encoding="utf-8") as f:
        f.write(
            "# Project\nname: MyApp\ndescription: D\npurpose: P\n"
            "stack: Python\nversion: unpinned\nstatus: draft\n"
            "\n[identity stabilized: t]\nstatus: stable\n"
        )
    with open(os.path.join(d, "app.py"), "w", encoding="utf-8") as f:
        f.write("print(1)\n")

    s1, _ = _build_system_prompt("## Task\ndo it", d)
    assert "Project Overview" in s1  # still injected, per design

    with open(card_path, "a", encoding="utf-8") as f:
        f.write("\n[updated: t2]\nstack: Python, Redis\n")
    s2, _ = _build_system_prompt("## Task\ndo it", d)
    assert s1 == s2


def test_plan_reinjection_newest_wins_policy():
    """At most ONE plan block survives any resume: identical skips,
    revisions replace, fully-completed plans disappear."""
    from langchain_core.messages import HumanMessage

    from agents.developer_agent import _apply_plan_reinjection

    def block(done, total, steps):
        lines = [f"=== CURRENT PLAN ({done}/{total} completed) ==="]
        marks = {"done": "[x]", "open": "[ ]"}
        seq = ["done"] * done + ["open"] * (total - done)
        for i, mk in zip(steps, seq):
            lines.append(f"  {mk} {i}")
        lines.append("=== END PLAN ===")
        return "\n".join(lines)

    v1 = block(0, 2, ["a", "b"])
    v2 = block(1, 2, ["a", "b"])
    fin = block(2, 2, ["a", "b"])

    def hist(*blocks):
        msgs = [HumanMessage(content=f"turn {i}") for i in range(3)]
        msgs += [HumanMessage(content=b) for b in blocks]
        return msgs

    is_plan = lambda x: x.content.startswith("=== CURRENT PLAN")  # noqa: E731

    m = hist(v1)
    _apply_plan_reinjection(m, v1)  # identical → skip entirely
    assert sum(1 for x in m if is_plan(x)) == 1

    m = hist(v1, v1)
    _apply_plan_reinjection(m, v2)  # revision replaces stacked copies
    plans = [x for x in m if is_plan(x)]
    assert len(plans) == 1 and plans[0].content == v2 and m[-1] is plans[0]

    m = hist(v1, v2)
    _apply_plan_reinjection(m, fin)  # model declared done → remove all
    assert not any(is_plan(x) for x in m)

    m = hist(v1)
    _apply_plan_reinjection(m, "")  # empty store strips stale block
    assert not any(is_plan(x) for x in m)

    m = hist()
    _apply_plan_reinjection(m, "")  # nothing anywhere → untouched
    assert len(m) == 3


if __name__ == "__main__":
    for fn in list(globals()):
        if fn.startswith("test_"):
            globals()[fn]()
            print(f"PASS {fn}")
