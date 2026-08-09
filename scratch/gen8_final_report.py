"""gen8_final_report.py — final report for the user-style gens 2,3,4,7 eval run.

Table 1: per-category totals (tasks, pass, pass@1, turns, input, cache hit, cache
         miss, output, reasoning, cost) from scratch/eval_runs/*.json reports.
Table 2: token-cause breakdown from .deep_agents/gen5_eval_calls.jsonl per-call
         telemetry (agent, reasoning chars, Dev iteration/think-cause, tool type).

Run:  venv312/Scripts/python.exe scratch/gen8_final_report.py
"""
import json
import os
import re
import sys

sys.stdout.reconfigure(encoding="utf-8")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUNS = os.path.join(ROOT, "scratch", "eval_runs")
CALLS = os.path.join(ROOT, ".deep_agents", "gen5_eval_calls.jsonl")

WANT = ("gen2-", "gen3-", "gen4-", "gen7-")


def load_reports():
    out = []
    for fn in sorted(os.listdir(RUNS)):
        if not fn.endswith(".json"):
            continue
        try:
            r = json.load(open(os.path.join(RUNS, fn), encoding="utf-8"))
        except Exception:
            continue
        if r.get("prompt_style") == "user" and r.get("category", "").startswith(WANT):
            out.append(r)
    return out


def fmt_k(n):
    return f"{n/1000:.1f}k" if n >= 1000 else f"{n:.0f}"


def table1(reports):
    cats = {}
    for r in reports:
        c = r["category"]
        cats.setdefault(c, []).append(r)
    rows = []
    for cat in sorted(cats):
        rs = cats[cat]
        agg = {
            "n": len(rs),
            "pass": sum(1 for r in rs if r["status"] == "pass"),
            "p1": sum(1 for r in rs if r.get("pass_at_1") == 1),
            "p1n": sum(1 for r in rs if r.get("pass_at_1") is not None),
            "turns": sum(r.get("paid_calls", 0) for r in rs),
            "inp": sum(r.get("input_tokens", 0) for r in rs),
            "hit": sum(r.get("cache_hit", 0) for r in rs),
            "miss": sum(r.get("cache_miss", 0) for r in rs),
            "out": sum(r.get("output_tokens", 0) for r in rs),
            "reason": sum(r.get("reasoning_tokens", 0) for r in rs),
            "cost": sum(r.get("cost", 0.0) for r in rs),
        }
        rows.append((cat, agg))
    tot = {k: sum(r[1][k] for r in rows) for k in ("n", "pass", "p1", "p1n", "turns",
                                                   "inp", "hit", "miss", "out", "reason", "cost")}
    rows.append(("TOTAL", tot))
    print("=" * 104)
    print(f"{'category':<16}{'n':>4}{'pass':>5}{'pass@1':>7}{'turns':>7}{'input':>9}"
          f"{'cacheHIT':>9}{'cacheMISS':>10}{'output':>9}{'reason':>8}{'cost$':>9}")
    print("-" * 104)
    for cat, a in rows:
        p1s = f"{a['p1']}/{a['p1n']}" if a["p1n"] else "-"
        print(f"{cat:<16}{a['n']:>4}{a['pass']:>5}{p1s:>7}{a['turns']:>7}"
              f"{fmt_k(a['inp']):>9}{fmt_k(a['hit']):>9}{fmt_k(a['miss']):>10}"
              f"{fmt_k(a['out']):>9}{fmt_k(a['reason']):>8}{a['cost']:>9.4f}")
    print("-" * 104)
    rate = 100.0 * tot["hit"] / tot["inp"] if tot["inp"] else 0
    print(f"cache hit rate: {rate:.1f}%   total cost: ${tot['cost']:.4f}   "
          f"pass@1: {tot['p1']}/{tot['p1n']}   pass@final: {tot['pass']}/{tot['n']}")
    print()


def load_calls():
    calls = []
    with open(CALLS, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                try:
                    calls.append(json.loads(line))
                except Exception:
                    pass
    return calls


def tool_of(where):
    m = re.match(r"Iter \d+/\d+:\s*([\w-]+)", where or "")
    return m.group(1) if m else (where or "")[:24]


def table2(calls):
    # ── per-agent ──
    agents = {}
    for c in calls:
        a = c.get("agent", "?")
        agents.setdefault(a, {"turns": 0, "out": 0, "rc": 0})
        agents[a]["turns"] += 1
        agents[a]["out"] += c.get("output", 0)
        agents[a]["rc"] += c.get("reasoning_chars", 0) or 0
    print("=" * 96)
    print("TABLE 2a — OUTPUT & REASONING BY AGENT")
    print("-" * 96)
    print(f"{'agent':<14}{'turns':>7}{'output':>10}{'reason_tok':>11}{'reason%':>9}"
          f"{'out/turn':>10}{'reason/turn':>12}")
    for a in sorted(agents, key=lambda x: -agents[x]["turns"]):
        d = agents[a]
        rt = d["rc"] // 4
        print(f"{a:<14}{d['turns']:>7}{fmt_k(d['out']):>10}{fmt_k(rt):>11}"
              f"{(100.0*rt/d['out'] if d['out'] else 0):>8.1f}%"
              f"{fmt_k(d['out']/d['turns']):>10}{fmt_k(rt/d['turns']):>12}")
    print()

    # ── Dev reasoning by cause (thinking ladder) ──
    dev = [c for c in calls if c.get("agent") == "Developer"]
    causes = {"iter-1 planning (effort high)": [0, 0, 0],
              "failure/repair-triggered (iter>1, thinking on)": [0, 0, 0],
              "routine execution (thinking disabled)": [0, 0, 0]}
    for c in dev:
        w = c.get("where", "") or ""
        m = re.match(r"Iter (\d+)/", w)
        it = int(m.group(1)) if m else 0
        rc = c.get("reasoning_chars", 0) or 0
        if it <= 1:
            key = "iter-1 planning (effort high)"
        elif rc > 0:
            key = "failure/repair-triggered (iter>1, thinking on)"
        else:
            key = "routine execution (thinking disabled)"
        causes[key][0] += 1
        causes[key][1] += c.get("output", 0)
        causes[key][2] += rc
    print("=" * 96)
    print("TABLE 2b — WHAT CAUSED THE REASONING (Developer, per thinking-ladder bucket)")
    print("-" * 96)
    print(f"{'cause':<46}{'turns':>7}{'output':>10}{'reason_tok':>11}{'reason%':>9}")
    for k, (t, o, rc) in causes.items():
        rt = rc // 4
        print(f"{k:<46}{t:>7}{fmt_k(o):>10}{fmt_k(rt):>11}"
              f"{(100.0*rt/o if o else 0):>8.1f}%")
    print()

    # ── Dev output by tool type ──
    by_tool = {}
    for c in dev:
        t = tool_of(c.get("where", ""))
        by_tool.setdefault(t, [0, 0, 0])
        by_tool[t][0] += 1
        by_tool[t][1] += c.get("output", 0)
        by_tool[t][2] += c.get("reasoning_chars", 0) or 0
    print("=" * 96)
    print("TABLE 2c — Developer output by tool type")
    print("-" * 96)
    print(f"{'tool':<26}{'turns':>7}{'output':>10}{'reason_tok':>11}{'reason%':>9}")
    for t, (n, o, rc) in sorted(by_tool.items(), key=lambda x: -x[1][1]):
        rt = rc // 4
        print(f"{t[:26]:<26}{n:>7}{fmt_k(o):>10}{fmt_k(rt):>11}"
              f"{(100.0*rt/o if o else 0):>8.1f}%")
    print()

    # ── output composition ──
    tot_out = sum(c.get("output", 0) for c in calls)
    tot_rc = sum(c.get("reasoning_chars", 0) or 0 for c in calls)
    tot_rt = tot_rc // 4
    non_reason = tot_out - tot_rt
    print("=" * 96)
    print("TABLE 2d — OUTPUT COMPOSITION (whole run)")
    print("-" * 96)
    print(f"{'component':<30}{'tokens':>10}{'share':>9}")
    print(f"{'reasoning (thinking, discarded)':<30}{fmt_k(tot_rt):>10}"
          f"{(100.0*tot_rt/tot_out):>8.1f}%")
    print(f"{'content + tool-call JSON':<30}{fmt_k(non_reason):>10}"
          f"{(100.0*non_reason/tot_out):>8.1f}%")
    print()


def main():
    reports = load_reports()
    print(f"[report] {len(reports)} user-style reports for gens 2,3,4,7\n")
    table1(reports)
    calls = load_calls()
    print(f"[report] {len(calls)} LLM calls in telemetry\n")
    table2(calls)


if __name__ == "__main__":
    main()
