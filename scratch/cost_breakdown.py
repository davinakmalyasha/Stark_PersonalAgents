import json, glob

INPUT_RATE = 0.14        # per 1M (cache miss)
CACHE_HIT_RATE = 0.0028  # per 1M
OUTPUT_RATE = 0.28       # per 1M

def cost(r):
    miss = r.get('cache_miss', 0) or 0
    hit = r.get('cache_hit', 0) or 0
    out = r.get('output_tokens', 0) or 0
    return (miss/1e6)*INPUT_RATE + (hit/1e6)*CACHE_HIT_RATE + (out/1e6)*OUTPUT_RATE

cats = ['gen2-bugfix','gen2-multifile','gen2-python','gen2-refactor','gen3-chain','gen3-log','gen3-long','gen3-trap','gen4-chain','gen4-log','gen4-long','gen4-trap']
by = {c: {'n':0,'turns':0,'in':0,'out':0,'reason':0,'hit':0,'miss':0,'cost':0.0} for c in cats}
tot = {'n':0,'turns':0,'in':0,'out':0,'reason':0,'hit':0,'miss':0,'cost':0.0}

def add(d, r):
    d['n'] += 1
    d['turns'] += r.get('paid_calls', 0) or 0
    d['in'] += r.get('input_tokens', 0) or 0
    d['out'] += r.get('output_tokens', 0) or 0
    d['reason'] += r.get('reasoning_tokens', 0) or 0
    d['hit'] += r.get('cache_hit', 0) or 0
    d['miss'] += r.get('cache_miss', 0) or 0
    d['cost'] += cost(r)

for f in glob.glob('scratch/eval_runs/*.json'):
    if 'events' in f:
        continue
    try:
        r = json.load(open(f, encoding='utf-8'))
    except Exception:
        continue
    c = r.get('category')
    if c in by:
        add(by[c], r)
        add(tot, r)

print('%-15s %5s %7s %12s %12s %10s %12s %10s %9s' % ('category','tasks','turns','input','output','reason','c_hit','c_miss','cost$'))
print('-'*108)
for c in cats:
    d = by[c]
    if d['n'] == 0:
        continue
    print('%-15s %5d %7d %12d %12d %10d %12d %10d %9.4f' % (c, d['n'], d['turns'], d['in'], d['out'], d['reason'], d['hit'], d['miss'], d['cost']))
print('-'*108)
print('%-15s %5d %7d %12d %12d %10d %12d %10d %9.4f' % ('TOTAL', tot['n'], tot['turns'], tot['in'], tot['out'], tot['reason'], tot['hit'], tot['miss'], tot['cost']))

print('\n=== cost breakdown with DeepSeek V4 Flash official pricing ===')
print('rates: cache-miss input $0.14/1M | cache-hit input $0.0028/1M | output $0.28/1M')
miss_c = (tot['miss']/1e6)*INPUT_RATE
hit_c = (tot['hit']/1e6)*CACHE_HIT_RATE
out_c = (tot['out']/1e6)*OUTPUT_RATE
tot_c = miss_c + hit_c + out_c
print('cache-MISS input: $%.4f  (%.2f%%)' % (miss_c, 100*miss_c/tot_c))
print('cache-HIT  input: $%.4f  (%.2f%%)' % (hit_c, 100*hit_c/tot_c))
print('output           : $%.4f  (%.2f%%)' % (out_c, 100*out_c/tot_c))
print('TOTAL (official) : $%.4f' % tot_c)

print('\n=== token composition ===')
print('input tokens      : %12d  (%.2f%%)' % (tot['in'], 100*tot['in']/(tot['in']+tot['out'])))
print('  of which cached : %12d  (%.2f%% of input)' % (tot['hit'], 100*tot['hit']/tot['in']))
print('output tokens     : %12d  (%.2f%%)' % (tot['out'], 100*tot['out']/(tot['in']+tot['out'])))
print('  of which reason : %12d  (%.2f%% of output)' % (tot['reason'], 100*tot['reason']/tot['out'] if tot['out'] else 0))
print('avg turns/task    : %.1f  |  avg input/task: %d  |  avg output/task: %d' % (tot['turns']/tot['n'], tot['in']/tot['n'], tot['out']/tot['n']))
