import json, glob, os, datetime

cats = ['gen2-bugfix','gen2-multifile','gen2-python','gen2-refactor','gen3-chain','gen3-log','gen3-long','gen3-trap','gen4-chain','gen4-log','gen4-long','gen4-trap']
by = {c: {'n':0,'pass':0,'fail':0,'error':0,'cost':0.0,'dur':0.0,'in':0,'out':0,'reason':0,'paid':0,'cache_hit':0,'cache_miss':0} for c in cats}
tot = {k: 0 for k in list(by.values())[0]}
now = datetime.datetime.now().timestamp()
for f in glob.glob('scratch/eval_runs/*.json'):
    if 'events' in f: continue
    try: r = json.load(open(f, encoding='utf-8'))
    except Exception: continue
    c = r.get('category')
    if c not in by: continue
    if now - os.path.getmtime(f) > 3600: continue   # only fresh reports from this regression
    d = by[c]
    d['n'] += 1
    st = r.get('status')
    if st == 'pass': d['pass'] += 1
    elif st == 'error': d['error'] += 1
    else: d['fail'] += 1
    for k in ['cost','dur','in','out','reason','paid','cache_hit','cache_miss']:
        d[k] += r.get({'cost':'cost','dur':'duration_s','in':'input_tokens','out':'output_tokens','reason':'reasoning_tokens','paid':'paid_calls','cache_hit':'cache_hit','cache_miss':'cache_miss'}[k], 0) or 0

hdr = '%-15s %5s %5s %5s %5s %9s %8s %10s %10s %8s %6s %10s %10s' % ('category','tasks','pass','fail','err','cost$','dur_min','input','output','reason','calls','c_hit','c_miss')
print(hdr)
print('-'*len(hdr))
for c in cats:
    d = by[c]
    if d['n'] == 0: continue
    print('%-15s %5d %5d %5d %5d %9.3f %8.1f %10d %10d %8d %6d %10d %10d' % (
        c, d['n'], d['pass'], d['fail'], d['error'], d['cost'], d['dur']/60,
        int(d['in']), int(d['out']), int(d['reason']), int(d['paid']), int(d['cache_hit']), int(d['cache_miss'])))
    for k in tot:
        tot[k] += d[k]
print('-'*len(hdr))
print('%-15s %5d %5d %5d %5d %9.3f %8.1f %10d %10d %8d %6d %10d %10d' % (
    'TOTAL', tot['n'], tot['pass'], tot['fail'], tot['error'], tot['cost'], tot['dur']/60,
    int(tot['in']), int(tot['out']), int(tot['reason']), int(tot['paid']), int(tot['cache_hit']), int(tot['cache_miss'])))
