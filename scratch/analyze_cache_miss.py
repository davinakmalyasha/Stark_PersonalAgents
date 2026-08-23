import os
import sys
import json
import collections

sys.stdout.reconfigure(encoding='utf-8')

log_file = os.path.join(os.getcwd(), ".deep_agents", "gen5_eval_calls.jsonl")

if not os.path.exists(log_file):
    print("No gen5_eval_calls.jsonl found.")
    sys.exit(0)

calls = []
for line in open(log_file, encoding='utf-8', errors='ignore'):
    line = line.strip()
    if line:
        try:
            calls.append(json.loads(line))
        except Exception:
            pass

print(f"Total calls logged in session: {len(calls)}")

total_hit = sum(c.get('cache_hit_tokens', 0) for c in calls)
total_miss = sum(c.get('cache_miss_tokens', 0) for c in calls)
total_out = sum(c.get('output_tokens', 0) for c in calls)
total_reasoning = sum((c.get('reasoning_chars', 0) or 0) // 4 for c in calls)
total_in = total_hit + total_miss

print(f"Total Input: {total_in:,} | Hit: {total_hit:,} ({total_hit/max(total_in,1)*100:.1f}%) | Miss: {total_miss:,} | Out: {total_out:,}")

# By Role Breakdown
by_role = collections.defaultdict(lambda: collections.Counter())
for c in calls:
    role = c.get('role') or c.get('agent') or 'unknown'
    by_role[role]['calls'] += 1
    by_role[role]['hit'] += c.get('cache_hit_tokens', 0)
    by_role[role]['miss'] += c.get('cache_miss_tokens', 0)
    by_role[role]['out'] += c.get('output_tokens', 0)
    by_role[role]['reasoning'] += (c.get('reasoning_chars', 0) or 0) // 4
    by_role[role]['cost'] += c.get('cost', 0) or 0

print("\n--- Breakdown by Role ---")
for r, stats in sorted(by_role.items()):
    r_in = stats['hit'] + stats['miss']
    hit_rate = stats['hit'] / max(r_in, 1) * 100
    print(f"Role: {r:<15} | Calls: {stats['calls']:<4} | Hit: {stats['hit']:>10,} ({hit_rate:>5.1f}%) | Miss: {stats['miss']:>8,} | Out: {stats['out']:>6,} | Cost: ${stats['cost']:.4f}")

# First call vs subsequent calls per task
by_task_first = collections.Counter()
by_task_later = collections.Counter()
tasks_seen = set()

for c in calls:
    task = c.get('task') or c.get('task_id') or 'unknown'
    is_first = task not in tasks_seen
    tasks_seen.add(task)
    target = by_task_first if is_first else by_task_later
    target['calls'] += 1
    target['hit'] += c.get('cache_hit_tokens', 0)
    target['miss'] += c.get('cache_miss_tokens', 0)

print("\n--- Turn 1 vs Subsequent Turns ---")
f_in = by_task_first['hit'] + by_task_first['miss']
l_in = by_task_later['hit'] + by_task_later['miss']
print(f"Turn 1 calls     : {by_task_first['calls']} calls | Hit: {by_task_first['hit']:,} ({by_task_first['hit']/max(f_in,1)*100:.1f}%) | Miss: {by_task_first['miss']:,}")
print(f"Subsequent turns : {by_task_later['calls']} calls | Hit: {by_task_later['hit']:,} ({by_task_later['hit']/max(l_in,1)*100:.1f}%) | Miss: {by_task_later['miss']:,}")

# Look at distribution of cache miss sizes
miss_sizes = [c.get('cache_miss_tokens', 0) for c in calls if c.get('cache_miss_tokens', 0) > 0]
if miss_sizes:
    print(f"\nCache miss distribution across {len(miss_sizes)} calls with misses:")
    print(f"  Min: {min(miss_sizes):,} | Max: {max(miss_sizes):,} | Avg: {sum(miss_sizes)/len(miss_sizes):,.0f} | Median: {sorted(miss_sizes)[len(miss_sizes)//2]:,}")
