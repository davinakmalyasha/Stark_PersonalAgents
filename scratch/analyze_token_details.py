import json
import os
import sys

sys.stdout.reconfigure(encoding='utf-8')

calls_file = r"d:\MyProject\LangChain\.deep_agents\gen5_eval_calls.jsonl"
if not os.path.exists(calls_file):
    print("gen5_eval_calls.jsonl not found.")
    sys.exit(0)

print("Analyzing token details of the latest calls...")
turns = []
with open(calls_file, "r", encoding="utf-8") as f:
    for line in f:
        try:
            turns.append(json.loads(line))
        except Exception:
            pass

# Filter calls from the most recent run (we can look at the latest 30 calls)
latest_turns = turns[-30:]

print(f"{'Turn':<4} | {'Agent':<15} | {'Model':<18} | {'Input':<6} | {'Output':<6} | {'Cache Hit':<9} | {'Cache Miss':<10} | {'Where'}")
print("-" * 120)
for idx, t in enumerate(latest_turns):
    turn_num = len(turns) - 30 + idx
    agent = t.get("agent", "")
    model = t.get("model", "")
    inp = t.get("input", 0)
    out = t.get("output", 0)
    hits = t.get("cache_hits", 0)
    misses = t.get("cache_misses", 0)
    where = t.get("where", "")
    # Shorten where
    where_short = where[:50].replace('\n', ' ')
    print(f"{turn_num:<4} | {agent:<15} | {model:<18} | {inp:<6} | {out:<6} | {hits:<9} | {misses:<10} | {where_short}")
