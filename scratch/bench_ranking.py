"""bench_ranking.py — old degree ranking vs new PageRank on THIS repo.

Read-only: generates maps in memory, touches no cache file.
Run: venv312/Scripts/python.exe scratch/bench_ranking.py
"""
import os
import sys
import time

sys.path.insert(0, r"D:\MyProject\LangChain")
sys.stdout.reconfigure(encoding="utf-8")

from memory.repo_map_generator import RepoMapGenerator, _is_utility_file  # noqa: E402

REPO = r"D:\MyProject\LangChain"


def main():
    gen = RepoMapGenerator(REPO)
    files = gen._files()
    graph = gen._graph(files)
    incoming = {p: 0 for p in files}
    for targets in graph.values():
        for t in targets:
            incoming[t] += 1

    def entry_like(p):
        stem = os.path.splitext(os.path.basename(p))[0].lower()
        return stem in gen.ENTRY_STEMS

    # ── OLD: degree formula (incoming*4 + outgoing*2 + known-name bonus) ──
    known_names = {
        "main.py", "app.py", "server.py", "api_server.py", "orchestrator_agent.py",
        "developer_agent.py", "memory_manager.py", "repo_map_generator.py",
        "package.json", "pyproject.toml"
    }
    old_ranked = sorted(
        files,
        key=lambda p: (
            -(incoming[p] * 4 + len(graph[p]) * 2
              + (10 if os.path.basename(p).lower() in known_names else 0)),
            p,
        ),
    )
    old_key = [p for p in old_ranked if incoming[p] or graph[p]
               or os.path.basename(p).lower() in known_names][:12]

    # ── NEW: PageRank + penalty + utility slot-cap ──
    t0 = time.perf_counter()
    scores = gen._pagerank_scores(files, graph)
    pr_ms = (time.perf_counter() - t0) * 1000
    new_ranked = sorted(files, key=lambda p: (-scores.get(p, 0.0), p))
    eligible = [p for p in new_ranked if incoming[p] or graph[p] or entry_like(p)]
    new_key, utils = [], 0
    for p in eligible:
        if _is_utility_file(p):
            if utils >= 3:
                continue
            utils += 1
        new_key.append(p)
        if len(new_key) == 12:
            break

    print(f"scanned {len(files)} files | PageRank: {pr_ms:.1f} ms")
    print(f"\n{'OLD (degree)':<44}NEW (PageRank+cap)")
    print("-" * 90)
    for i in range(max(len(old_key), len(new_key))):
        o = old_key[i] if i < len(old_key) else ""
        n = new_key[i] if i < len(new_key) else ""
        mark_o = " [UTIL]" if o and _is_utility_file(o) else ""
        mark_n = " [UTIL]" if n and _is_utility_file(n) else ""
        print(f"{i+1:>2}. {o[:38]}{mark_o:<7} | {n[:38]}{mark_n}")

    t0 = time.perf_counter()
    pairs = gen._cochange_pairs(files)
    cc_ms = (time.perf_counter() - t0) * 1000
    print(f"\nco-change pairs ({len(pairs)}, {cc_ms:.0f} ms incl. git log):")
    for line in pairs:
        print(" ", line)

    t0 = time.perf_counter()
    code_map = gen.generate_map()
    total_ms = (time.perf_counter() - t0) * 1000
    print(f"\ngenerate_map total: {total_ms:.0f} ms, {len(code_map)} chars")
    print("=" * 90)
    print(code_map)


if __name__ == "__main__":
    main()
