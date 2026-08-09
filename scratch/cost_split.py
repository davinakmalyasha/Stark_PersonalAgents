"""Show cost split: input (cache-miss vs hit) vs output per role."""
import json
import glob
import sys
import collections

sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def load_usage():
    calls = []
    for f in glob.glob(".deep_agents/chats/*/*_usage.json"):
        try:
            d = json.load(open(f, encoding="utf-8"))
            calls.extend(d.get("calls", []))
        except Exception:
            pass
    return calls


# DeepSeek flash rates
IN_MISS = 0.14
IN_HIT = 0.0028
OUT = 0.28
PRO_IN_MISS = 0.435
PRO_IN_HIT = 0.003625
PRO_OUT = 0.87


def rates(model):
    if "pro" in model:
        return PRO_IN_MISS, PRO_IN_HIT, PRO_OUT
    return IN_MISS, IN_HIT, OUT


def main():
    calls = load_usage()
    agg = collections.defaultdict(lambda: {"out": 0, "in_miss": 0, "in_hit": 0, "out_cost": 0.0, "in_cost": 0.0, "n": 0})
    for c in calls:
        a = c.get("agent", "?")
        r = rates(c.get("model", ""))
        in_t = c.get("input") or 0
        hit = c.get("cache_hits") or 0
        out_t = c.get("output") or 0
        agg[a]["out"] += out_t
        agg[a]["in_miss"] += max(0, in_t - hit)
        agg[a]["in_hit"] += hit
        agg[a]["out_cost"] += out_t / 1e6 * r[2]
        agg[a]["in_cost"] += (max(0, in_t - hit) / 1e6 * r[0]) + (hit / 1e6 * r[1])
        agg[a]["n"] += 1

    print(f"{'ROLE':<14}{'OUT_TOK':>10}{'OUT_COST':>10}{'IN_COST':>10}{'COST_TOT':>10}{'OUT_%':>7}")
    tot_out_c = tot_in_c = 0
    for a, v in sorted(agg.items(), key=lambda x: -(x[1]["out_cost"] + x[1]["in_cost"])):
        out_pct = 100.0 * v["out_cost"] / max(1e-9, v["out_cost"] + v["in_cost"])
        tot_out_c += v["out_cost"]
        tot_in_c += v["in_cost"]
        print(f"{a:<14}{v['out']:>10,}${v['out_cost']:>9.4f}${v['in_cost']:>9.4f}${v['out_cost']+v['in_cost']:>9.4f}{out_pct:>6.1f}%")
    print(f"\nTOTAL: output ${tot_out_c:.4f} | input ${tot_in_c:.4f} | "
          f"output = {100.0*tot_out_c/max(1,tot_out_c+tot_in_c):.1f}% of combined cost")
    print("(flash cache-hit input is $0.0028/1M, output is $0.28/1M — output is 100x the cached-input rate)")


if __name__ == "__main__":
    main()
