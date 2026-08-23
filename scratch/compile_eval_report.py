import os
import sys
import json
import glob
import collections

sys.stdout.reconfigure(encoding='utf-8')

cats = [
    'gen2-bugfix', 'gen2-multifile', 'gen2-python', 'gen2-refactor',
    'gen3-chain', 'gen3-log', 'gen3-long', 'gen3-trap',
    'gen4-chain', 'gen4-log', 'gen4-long', 'gen4-trap',
    'gen7-ops'
]

files = sorted(glob.glob('scratch/eval_runs/*.json'))
reports = []

for f in files:
    if '_events' in f:
        continue
    try:
        r = json.load(open(f, encoding='utf-8'))
        if r.get('category') in cats and r.get('status') in ('pass', 'fail', 'error'):
            reports.append(r)
    except Exception:
        pass

total_tasks = len(reports)
pass_final = sum(1 for r in reports if r.get('status') == 'pass')
pass_at_1 = sum(1 for r in reports if r.get('pass_at_1') == 1)
total_cost = sum(r.get('cost', 0) or 0 for r in reports)
cost_per_task = total_cost / max(total_tasks, 1)
total_calls = sum(r.get('paid_calls', 0) or 0 for r in reports)
avg_calls = total_calls / max(total_tasks, 1)
input_tokens = sum(r.get('input_tokens', 0) or 0 for r in reports)
cache_miss = sum(r.get('cache_miss', 0) or 0 for r in reports)
cache_hit = sum(r.get('cache_hit', 0) or 0 for r in reports)
output_tokens = sum(r.get('output_tokens', 0) or 0 for r in reports)
reasoning_tokens = sum(r.get('reasoning_tokens', 0) or 0 for r in reports)

print(f"=== 183-TASK EVALUATION METRICS ===")
print(f"Total Tasks: {total_tasks}")
print(f"Pass@final: {pass_final} / {total_tasks} ({pass_final/total_tasks*100:.1f}%)")
print(f"Pass@1: {pass_at_1} / {total_tasks} ({pass_at_1/total_tasks*100:.1f}%)")
print(f"Total Cost: ${total_cost:.4f}")
print(f"Cost / Task: ${cost_per_task:.5f}")
print(f"Total Calls (avg/task): {avg_calls:.3f} (total: {total_calls})")
print(f"Input Tokens: {input_tokens:,} ({input_tokens/1e6:.2f}M)")
print(f"Cache MISS: {cache_miss:,} ({cache_miss/1000:.1f}k)")
print(f"Cache HIT: {cache_hit:,} ({cache_hit/1e6:.2f}M)")
print(f"Output Tokens: {output_tokens:,} ({output_tokens/1000:.1f}k)")
print(f"Reasoning Tokens: {reasoning_tokens:,} ({reasoning_tokens/1000:.1f}k)")

# Print category breakdown
print("\n--- Category Breakdown ---")
by_cat = collections.defaultdict(lambda: collections.Counter())
for r in reports:
    c = r.get('category')
    by_cat[c]['total'] += 1
    if r.get('status') == 'pass':
        by_cat[c]['pass'] += 1
    if r.get('pass_at_1') == 1:
        by_cat[c]['pass1'] += 1
    by_cat[c]['cost'] += r.get('cost', 0) or 0

for c, stats in sorted(by_cat.items()):
    print(f"  {c:<16}: pass={stats['pass']}/{stats['total']} | pass@1={stats['pass1']}/{stats['total']} | cost=${stats['cost']:.4f}")

# Check failed tasks
failed_tasks = [r for r in reports if r.get('status') != 'pass']
if failed_tasks:
    print(f"\n--- Failed Tasks ({len(failed_tasks)}) ---")
    for r in failed_tasks:
        print(f"  {r.get('id')} ({r.get('category')}): {r.get('test_detail', '')[:100]}")
