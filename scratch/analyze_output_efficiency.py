"""Analyze what drives high output tokens in the harness.

Two goals:
1. Confirm whether large per-call outputs are reasoning_content (billed, discarded)
   vs actual content (used).
2. Find the biggest single-output call sites.
"""
import json
import glob
import collections
import sys

CHATS = ".deep_agents/chats/*/*_usage.json"


def load_usage():
    calls = []
    for f in glob.glob(CHATS):
        try:
            d = json.load(open(f, encoding="utf-8"))
            calls.extend(d.get("calls", []))
        except Exception:
            pass
    return calls


def main():
    calls = load_usage()
    print(f"Total LLM call records: {len(calls)}")
    total_out = sum(c.get("output") or 0 for c in calls)
    total_in = sum(c.get("input") or 0 for c in calls)
    print(f"Total output: {total_out:,}  total input: {total_in:,}  "
          f"(output is {100.0*total_out/max(1,total_in):.1f}% of input)")

    # Big single calls
    have_reasoning = any(c.get("reasoning_chars") for c in calls)
    print("\n=== TOP 15 single calls by output tokens ===")
    for c in sorted(calls, key=lambda x: -(x.get("output") or 0))[:15]:
        rc = c.get("reasoning_chars", 0) or 0
        rc_tok = rc // 4
        rtag = f"  reasoning≈{rc_tok:,}tok" if have_reasoning else ""
        print(f"{c.get('output'):>8,} out | {c.get('input'):>8,} in | "
              f"{c.get('agent'):<12} | {(c.get('where') or '')[:45]}{rtag}")

    if have_reasoning:
        tot_rc = sum((c.get("reasoning_chars") or 0) for c in calls)
        tot_out = sum((c.get("output") or 0) for c in calls)
        print(f"\nTotal reasoning chars: {tot_rc:,}  (~{tot_rc//4:,} tokens = "
              f"{100.0*(tot_rc/4)/max(1,tot_out):.0f}% of total output)")
    else:
        print("\n(no reasoning_chars field yet — run once after the effort-ladder "
              "change to populate it)")

    # Output >= 2000 tokens (extreme outliers)
    big = [c for c in calls if (c.get("output") or 0) >= 2000]
    print(f"\nCalls with output >= 2,000 tokens: {len(big)}")
    for c in sorted(big, key=lambda x: -x.get("output", 0)):
        print(f"  {c.get('output'):>7,} | {c.get('agent'):<12} | "
              f"{c.get('model'):<18} | {(c.get('where') or '')[:60]}")

    # Reasoning likelihood: Developer/Orchestrator have thinking enabled.
    reasoning_roles = {"Developer", "Orchestrator", "Supervisor"}
    print("\n=== Output by role ===")
    for r in sorted(set(c.get("agent") for c in calls)):
        tot = sum(c.get("output") or 0 for c in calls if c.get("agent") == r)
        n = sum(1 for c in calls if c.get("agent") == r)
        pct = 100.0 * tot / max(1, total_out)
        tag = "  <-- thinking ENABLED (high)" if r in reasoning_roles else ""
        print(f"{r:<14} avg={tot//max(1,n):>6,}/call  total={tot:>9,} ({pct:4.1f}%){tag}")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    main()
