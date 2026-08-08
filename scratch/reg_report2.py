import json, glob, os, datetime
now = datetime.datetime.now().timestamp()
for f in sorted(glob.glob('scratch/eval_runs/*.json')):
    if 'events' in f:
        continue
    try:
        r = json.load(open(f, encoding='utf-8'))
    except Exception:
        continue
    if r.get('status') != 'pass':
        age_min = (now - os.path.getmtime(f)) / 60
        print('%-30s %-12s %-8s age=%.0fmin hollow_retries=%s attempts=%s repairs=%s cost=$%.4f dur=%.0fs' % (
            r.get('id'), r.get('category'), r.get('status'), age_min, r.get('hollow_retries'),
            r.get('attempts'), r.get('repairs'), r.get('cost') or 0, r.get('duration_s') or 0))
        print('    detail: %s' % str(r.get('test_detail') or '')[:200])

print('\n--- HOLLOW EVENTS ---')
n = 0
for line in open('scratch/eval_runs/_events.jsonl', encoding='utf-8'):
    try:
        e = json.loads(line)
    except Exception:
        continue
    if e.get('event') == 'hollow':
        n += 1
        print('  %-28s retry=%s work=%s cost_delta=%s calls=%s detail=%s' % (
            e.get('id'), e.get('retry'), e.get('work'), e.get('cost_delta'),
            e.get('calls_delta'), str(e.get('detail') or '')[:60]))
print('total hollow events:', n)
