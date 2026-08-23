import os
import sys
import json
import glob
import collections

sys.stdout.reconfigure(encoding='utf-8')

files = sorted(glob.glob('scratch/eval_runs/*.json'))
reports = []

for f in files:
    if '_events' in f:
        continue
    try:
        r = json.load(open(f, encoding='utf-8'))
        reports.append(r)
    except Exception:
        pass

print(f"Total reports found in scratch/eval_runs: {len(reports)}")

total_hit = sum(r.get('cache_hit', 0) or 0 for r in reports)
total_miss = sum(r.get('cache_miss', 0) or 0 for r in reports)
total_out = sum(r.get('output_tokens', 0) or 0 for r in reports)
total_in = total_hit + total_miss
total_cost = sum(r.get('cost', 0) or 0 for r in reports)
total_calls = sum(r.get('paid_calls', 0) or 0 for r in reports)

print(f"Total Input: {total_in:,}")
print(f"  - Cache Hit : {total_hit:,} ({total_hit/max(total_in, 1)*100:.1f}%)")
print(f"  - Cache Miss: {total_miss:,} ({total_miss/max(total_in, 1)*100:.1f}%)")
print(f"Total Output : {total_out:,}")
print(f"Total Paid Calls: {total_calls:,}")
print(f"Total Cost   : ${total_cost:.4f}")

# Group by category
print("\n--- Per Category Cache Statistics ---")
by_cat = collections.defaultdict(lambda: collections.Counter())
for r in reports:
    c = r.get('category', 'unknown')
    by_cat[c]['n'] += 1
    by_cat[c]['hit'] += r.get('cache_hit', 0) or 0
    by_cat[c]['miss'] += r.get('cache_miss', 0) or 0
    by_cat[c]['out'] += r.get('output_tokens', 0) or 0
    by_cat[c]['cost'] += r.get('cost', 0) or 0
    by_cat[c]['calls'] += r.get('paid_calls', 0) or 0

for c, s in sorted(by_cat.items()):
    cin = s['hit'] + s['miss']
    hr = s['hit'] / max(cin, 1) * 100
    avg_miss = s['miss'] / max(s['n'], 1)
    print(f"{c:<16}: tasks={s['n']:<3} | hit={s['hit']:>10,} ({hr:>5.1f}%) | miss={s['miss']:>8,} (avg/task={avg_miss:>6,.0f}) | calls={s['calls']:<4} | cost=${s['cost']:.4f}")

# Sample tasks and check miss per task
print("\n--- Sample 10 tasks (Hit vs Miss) ---")
for r in reports[:10]:
    t_in = (r.get('cache_hit', 0) or 0) + (r.get('cache_miss', 0) or 0)
    t_hr = (r.get('cache_hit', 0) or 0) / max(t_in, 1) * 100
    print(f"  {r.get('id'):<24} ({r.get('category')}): calls={r.get('paid_calls',0):<2} | in={t_in:>7,} | hit={r.get('cache_hit',0):>7,} ({t_hr:>5.1f}%) | miss={r.get('cache_miss',0):>6,}")
