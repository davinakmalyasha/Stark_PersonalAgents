import json, glob, os, re

# 1) The failing task's full event trail
print("=== g4l-stateful-crash trail (this session) ===")
for line in open('scratch/eval_runs/_events.jsonl', encoding='utf-8'):
    try: e = json.loads(line)
    except Exception: continue
    if e.get('id') == 'g4l-stateful-crash' and e.get('ts') > 1785766000:
        print('  %.1f %-12s %s %s' % (e.get('ts'), e.get('event'), e.get('attempt',''), str(e.get('detail') or e.get('reason') or '')[:90]))

# 2) Provider / LLM errors across all regression logs
print("\n=== provider/LLM 500+ errors in regression logs ===")
errs = {}
pat = re.compile(r'Error code: (\d+)')
for f in glob.glob('scratch/eval_runs/_reg*.log') + glob.glob('scratch/eval_runs/_reg2*.log') + glob.glob('scratch/eval_runs/_reg3*.log'):
    try:
        txt = open(f, encoding='utf-8', errors='ignore').read()
    except Exception:
        continue
    for m in pat.findall(txt):
        key = 'HTTP ' + m
        errs[key] = errs.get(key, 0) + 1
    for kw in ['All DeepSeek models exhausted', 'Empty response, trying next', 'Internal server error']:
        c = txt.count(kw)
        if c:
            errs[kw] = errs.get(kw, 0) + c
for k, v in sorted(errs.items(), key=lambda x: -x[1]):
    print('  %-45s %d' % (k, v))

# 3) All hollow events
print("\n=== all hollow events ===")
for line in open('scratch/eval_runs/_events.jsonl', encoding='utf-8'):
    try: e = json.loads(line)
    except Exception: continue
    if e.get('event') == 'hollow':
        print('  %-28s retry=%s cost=$%s calls=%s' % (e.get('id'), e.get('retry'), e.get('cost_delta'), e.get('calls_delta')))

# 4) All attempt-level hollow flags
print("\n=== attempts flagged hollow ===")
for line in open('scratch/eval_runs/_events.jsonl', encoding='utf-8'):
    try: e = json.loads(line)
    except Exception: continue
    if e.get('event') == 'attempt' and e.get('hollow'):
        print('  %-28s attempt=%s work=%s cost=$%s calls=%s' % (e.get('id'), e.get('attempt'), e.get('work'), e.get('cost_delta'), e.get('calls_delta')))

# 5) Session totals from all reports (fresh enough, this session only)
print("\n=== session token/cost totals (all gen2-4 reports touched today) ===")
tot = {'cost':0,'dur':0,'in':0,'out':0,'reason':0,'paid':0,'n':0}
for f in glob.glob('scratch/eval_runs/*.json'):
    if 'events' in f: continue
    try: r = json.load(open(f, encoding='utf-8'))
    except Exception: continue
    if r.get('category','').startswith('gen') and r.get('duration_s'):
        tot['cost'] += r.get('cost',0) or 0
        tot['dur'] += r.get('duration_s',0) or 0
        tot['in'] += r.get('input_tokens',0) or 0
        tot['out'] += r.get('output_tokens',0) or 0
        tot['reason'] += r.get('reasoning_tokens',0) or 0
        tot['paid'] += r.get('paid_calls',0) or 0
        tot['n'] += 1
print('  tasks=%d cost=$%.3f dur=%.1fh input=%d output=%d reasoning=%d paid_calls=%d' % (
    tot['n'], tot['cost'], tot['dur']/3600, tot['in'], tot['out'], tot['reason'], tot['paid']))
