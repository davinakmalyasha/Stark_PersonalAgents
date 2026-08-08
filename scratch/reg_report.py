import json, glob, os, datetime

cats = ['gen2-bugfix','gen2-multifile','gen2-python','gen2-refactor','gen3-chain','gen3-log','gen3-long','gen3-trap','gen4-chain','gen4-log','gen4-long','gen4-trap']
by = {c: {'pass':0,'fail':0,'cost':0.0,'dur':0.0,'n':0,'fresh':0} for c in cats}
now = datetime.datetime.now().timestamp()
for f in glob.glob('scratch/eval_runs/*.json'):
    if 'events' in f:
        continue
    try:
        r = json.load(open(f, encoding='utf-8'))
    except Exception:
        continue
    c = r.get('category')
    if c not in by:
        continue
    st = r.get('status')
    if st == 'pass':
        by[c]['pass'] += 1
    else:
        by[c]['fail'] += 1
    by[c]['n'] += 1
    by[c]['cost'] += r.get('cost', 0)
    by[c]['dur'] += r.get('duration_s', 0)
    if now - os.path.getmtime(f) < 3600:
        by[c]['fresh'] += 1

tot = {'pass':0,'fail':0,'cost':0.0,'dur':0.0,'n':0,'fresh':0}
for c, d in by.items():
    for k in tot:
        tot[k] += d[k]
    print('%-14s %3d/%3d pass  fail=%-3d cost=$%.3f  dur=%.1fmin  fresh=%d' % (c, d['pass'], d['n'], d['fail'], d['cost'], d['dur']/60, d['fresh']))
print('-'*90)
print('TOTAL       %3d/%3d pass  fail=%-3d cost=$%.3f  dur=%.1fmin  fresh=%d' % (tot['pass'], tot['n'], tot['fail'], tot['cost'], tot['dur']/60, tot['fresh']))
