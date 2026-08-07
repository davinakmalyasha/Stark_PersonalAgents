"""Report for the 183-task eval run: per-task table + per-role token/miss/turn breakdown.

Reads scratch/eval_runs/*.json (per-task reports) and .deep_agents/gen5_eval_calls.jsonl
(per-call telemetry). Prints:
  1. Per-task table (status, pass_at, attempts, repairs, cost, in/hit/miss/out, reasoning, calls, dur)
  2. Run totals (cost, miss, hit, output, pass@1, pass@final)
  3. Per-role breakdown (calls/turns, hit, miss, output, reasoning, effort split)
  4. Miss causes per role (Dev by tool tag + first-call vs routine; flip cost estimate)
"""
import os
import sys
import json
import glob
import collections

sys.stdout.reconfigure(encoding="utf-8")
BASE = r"D:\MyProject\LangChain"
RUNS = os.path.join(BASE, "scratch", "eval_runs")
CALLS = os.path.join(BASE, ".deep_agents", "gen5_eval_calls.jsonl")


def main():
    files = sorted(glob.glob(os.path.join(RUNS, "*.json")))
    reports = []
    for f in files:
        try:
            r = json.load(open(f, encoding="utf-8"))
            if r.get("status") in ("pass", "fail", "error"):
                reports.append(r)
        except Exception:
            pass
    reports.sort(key=lambda r: r.get("id", ""))

    print("=" * 120)
    print("PER-TASK TABLE")
    print("=" * 120)
    hdr = (f"{'id':<26}{'cat':<12}{'res':<6}{'pa':<7}{'att':<4}{'rep':<4}"
           f"{'cost$':<9}{'in':>9}{'hit':>10}{'miss':>9}{'out':>8}{'re':>6}{'calls':>6}{'dur':>7}")
    print(hdr)
    print("-" * len(hdr))
    tot = collections.Counter()
    for r in reports:
        st = r.get("status", "?")
        pa = r.get("pass_at")
        pa_s = f"@{pa}" if pa else "-"
        print(f"{r.get('id',''):<26}{str(r.get('category',''))[:11]:<12}{st:<6}{pa_s:<7}"
              f"{r.get('attempts',0):<4}{r.get('repairs',0):<4}"
              f"${r.get('cost',0):<8.4f}"
              f"{r.get('input_tokens',0):>9,}{r.get('cache_hit',0):>10,}{r.get('cache_miss',0):>9,}"
              f"{r.get('output_tokens',0):>8,}{r.get('reasoning_tokens',0):>6,}{r.get('paid_calls',0):>6}"
              f"{r.get('duration_s',0):>7.0f}")
        for k in ("status",):
            tot[k] += 1
        tot["cost"] += r.get("cost", 0) or 0
        tot["in"] += r.get("input_tokens", 0) or 0
        tot["hit"] += r.get("cache_hit", 0) or 0
        tot["miss"] += r.get("cache_miss", 0) or 0
        tot["out"] += r.get("output_tokens", 0) or 0
        tot["re"] += r.get("reasoning_tokens", 0) or 0
        tot["calls"] += r.get("paid_calls", 0) or 0
        if r.get("pass_at_1") == 1:
            tot["pass1"] += 1
        tot["n"] += 1
        tot["pass"] += 1 if st == "pass" else 0
        tot["fail"] += 1 if st == "fail" else 0
        tot["error"] += 1 if st == "error" else 0

    print("-" * len(hdr))
    n = tot.get("n", 1)
    print(f"TOTALS  tasks={tot['n']}  pass={tot['pass']}  fail={tot['fail']}  error={tot['error']}  "
          f"pass@1={tot.get('pass1',0)}/{tot['n']} "
          f"({100.0*tot.get('pass1',0)/max(n,1):.1f}%)  pass@final={tot['pass']}/{tot['n']} "
          f"({100.0*tot['pass']/max(n,1):.1f}%)")
    print(f"cost=${tot['cost']:.4f}  input={tot['in']:,}  hit={tot['hit']:,}  miss={tot['miss']:,}  "
          f"output={tot['out']:,}  reasoning_tokens={tot['re']:,}  calls={tot['calls']:,}  "
          f"avg_cost/task=${tot['cost']/max(n,1):.5f}")
    per_task_miss = tot["miss"] / max(n, 1)
    per_task_out = tot["out"] / max(n, 1)
    print(f"per-task avg: miss={per_task_miss:,.0f}  output={per_task_out:,.0f}  "
          f"miss-caused cost=${per_task_miss*0.14/1e6:.5f}/task  output cost=${per_task_out*0.28/1e6:.5f}/task")

    # ---- per-role from call log ----
    rows = []
    if os.path.exists(CALLS):
        for line in open(CALLS, encoding="utf-8", errors="ignore"):
            line = line.strip()
            if not line:
                continue
            try:
                rows.append(json.loads(line))
            except Exception:
                pass
    print()
    print("=" * 120)
    print("PER-ROLE BREAKDOWN (per-call telemetry)")
    print("=" * 120)
    if not rows:
        print("(no call log yet)")
    else:
        by_agent = collections.defaultdict(lambda: collections.Counter())
        for r in rows:
            a = r.get("agent", "?")
            c = by_agent[a]
            c["calls"] += 1
            for k in ("input", "cache_hits", "cache_misses", "output", "reasoning_chars"):
                v = r.get(k, 0)
                if isinstance(v, (int, float)):
                    c[k] += v
            c["eff_" + str(r.get("reasoning_effort", "?"))] += 1
        hdr = (f"{'agent':<14}{'calls':>6}{'turns':>6}{'hit':>11}{'miss':>10}{'miss/call':>10}"
               f"{'out':>9}{'re(ch)':>9}{'efforts':>28}")
        print(hdr)
        print("-" * len(hdr))
        for a, c in sorted(by_agent.items()):
            calls = c["calls"]
            effs = ", ".join(f"{k[4:]}={v}" for k, v in sorted(c.items()) if k.startswith("eff_"))
            print(f"{a:<14}{calls:>6}{'':>6}{c['cache_hits']:>11,}{c['cache_misses']:>10,}"
                  f"{c['cache_misses']/max(calls,1):>10,.0f}{c['output']:>9,}{c['reasoning_chars']:>9,}"
                  f"{effs:>28}")

        # ---- miss causes for Developer ----
        print()
        print("DEV MISS CAUSES (where-tag tool of the PREVIOUS turn's batch)")
        import re as _re
        pat = _re.compile(r"Iter \d+/\d+: (\w+)")
        dev = [r for r in rows if r.get("agent") == "Developer"]
        by_tool = collections.defaultdict(lambda: [0, 0.0])
        first_miss = 0.0
        n_first = 0
        rest_miss = 0.0
        n_rest = 0
        for r in dev:
            m = pat.search(r.get("where", ""))
            t = m.group(1) if m else "(other)"
            by_tool[t][0] += 1
            by_tool[t][1] += r.get("cache_misses", 0)
        for t, (c, m) in sorted(by_tool.items(), key=lambda kv: -kv[1][1]):
            print(f"  {t:<22} calls={c:>4}  miss={m:>10,.0f}  avg={m/max(c,1):>7,.0f}")

        # flip estimate: first Dev call of each where-sequence vs rest
        seq = collections.defaultdict(list)
        for r in dev:
            seq[r.get("where", "")].append(r)
        # approximate first-call vs routine using iteration number embedded in where
        n_iter1 = sum(1 for r in dev if _re.search(r"Iter 1/\d+", r.get("where", "")))
        miss_iter1 = sum(r.get("cache_misses", 0) for r in dev if _re.search(r"Iter 1/\d+", r.get("where", "")))
        n_other = sum(1 for r in dev if not _re.search(r"Iter 1/\d+", r.get("where", "")))
        miss_other = sum(r.get("cache_misses", 0) for r in dev if not _re.search(r"Iter 1/\d+", r.get("where", "")))
        print(f"\n  Dev iter-1 calls: n={n_iter1} miss={miss_iter1:,.0f} ({miss_iter1/max(n_iter1,1):,.0f}/call)")
        print(f"  Dev iter-2+ calls: n={n_other} miss={miss_other:,.0f} ({miss_other/max(n_other,1):,.0f}/call)")
        print("  -> iter-1 fresh content (task+volatile+flip) vs routine tool-result deltas")

        # reasoning output share
        tot_re_chars = sum(r.get("reasoning_chars", 0) for r in rows)
        tot_out = sum(r.get("output", 0) for r in rows)
        print(f"\n  output tokens total: {tot_out:,}  | reasoning chars: {tot_re_chars:,} "
              f"(~{tot_re_chars//4:,} tokens = {100.0*(tot_re_chars/4)/max(tot_out,1):.0f}% of output)")


if __name__ == "__main__":
    main()
