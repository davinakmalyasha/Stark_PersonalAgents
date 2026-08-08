import json, glob, os

# Full 13-category regression from current reports
cats = ['gen2-bugfix','gen2-multifile','gen2-python','gen2-refactor','gen3-chain','gen3-log','gen3-long','gen3-trap','gen4-chain','gen4-log','gen4-long','gen4-trap','gen7-ops']
rows = []
tot = {'n':0,'pass':0,'fail':0,'err':0,'cost':0.0,'dur':0.0,'in':0,'out':0,'reason':0,'paid':0}
pass1 = {'n':0,'ok':0}
for f in glob.glob('scratch/eval_runs/*.json'):
    if 'events' in f: continue
    try: r = json.load(open(f, encoding='utf-8'))
    except Exception: continue
    c = r.get('category')
    if c not in cats: continue
    st = r.get('status')
    d = {'id':r.get('id'),'cat':c,'st':st,'cost':r.get('cost',0) or 0,'dur':r.get('duration_s',0) or 0,
         'in':r.get('input_tokens',0) or 0,'out':r.get('output_tokens',0) or 0,
         'reason':r.get('reasoning_tokens',0) or 0,'paid':r.get('paid_calls',0) or 0,
         'hollow':r.get('hollow_retries',0) or 0,
         'pass1':r.get('pass_at_1'),'style':r.get('prompt_style','spec')}
    rows.append(d)
    tot['n'] += 1
    if st == 'pass': tot['pass'] += 1
    elif st == 'error': tot['err'] += 1
    else: tot['fail'] += 1
    tot['cost'] += d['cost']; tot['dur'] += d['dur']
    tot['in'] += d['in']; tot['out'] += d['out']; tot['reason'] += d['reason']; tot['paid'] += d['paid']
    if d['pass1'] is not None:
        pass1['n'] += 1
        pass1['ok'] += d['pass1'] == 1

print('=== GEN2-7 REGRESSION (13 categories, 183 tasks) ===')
print('%-28s %-12s %6s %9s %8s %10s %10s %8s %6s' % ('task','cat','status','cost$','dur_s','input','output','reason','calls'))
for d in sorted(rows, key=lambda x: x['cat']):
    print('%-28s %-12s %6s %9.4f %8.1f %10d %10d %8d %6d' % (d['id'], d['cat'], d['st'], d['cost'], d['dur'], d['in'], d['out'], d['reason'], d['paid']))
print('-'*100)
print('TOTAL: %d/%d pass, %d fail, %d error | cost $%.4f | %.1f min | in=%d out=%d reason=%d calls=%d' % (
    tot['pass'], tot['n'], tot['fail'], tot['err'], tot['cost'], tot['dur']/60, tot['in'], tot['out'], tot['reason'], tot['paid']))
if pass1['n']:
    print('pass@1: %d/%d (%.1f%%)   pass@final: %d/%d (%.1f%%)' % (
        pass1['ok'], pass1['n'], 100.0*pass1['ok']/pass1['n'],
        tot['pass'], tot['n'], 100.0*tot['pass']/tot['n']))
    styles = {}
    for d in rows:
        styles.setdefault(d['style'], {'n':0,'ok':0,'pass':0})
        styles[d['style']]['n'] += 1
        styles[d['style']]['pass'] += d['st'] == 'pass'
        if d['pass1'] is not None:
            styles[d['style']]['ok'] += d['pass1'] == 1
    for s, v in sorted(styles.items()):
        print('  [%s style] pass@1 %d/%d (%.1f%%) | pass@final %d/%d (%.1f%%)' % (
            s, v['ok'], v['n'], 100.0*v['ok']/v['n'] if v['n'] else 0,
            v['pass'], v['n'], 100.0*v['pass']/v['n'] if v['n'] else 0))

print('\n=== TASKS WITH HOLLOW RETRIES ===')
for d in sorted(rows, key=lambda x: -x['hollow']):
    if d['hollow']:
        print('  %-28s hollow_retries=%d status=%s' % (d['id'], d['hollow'], d['st']))

# gen6 pass
print('\n=== GEN6 (capstone) ===')
try:
    g6 = json.load(open('scratch/eval_runs/g6-ecommerce-store.json', encoding='utf-8'))
    print('  status=%s pass_at=%s attempts=%s repairs=%s cost=$%.4f dur=%.1fs in=%d out=%d reason=%d' % (
        g6['status'], g6.get('pass_at'), g6.get('attempts'), g6.get('repairs'), g6.get('cost',0), g6.get('duration_s',0),
        g6.get('input_tokens',0), g6.get('output_tokens',0), g6.get('reasoning_tokens',0)))
except Exception as e:
    print('  err', e)

# gen5
print('\n=== GEN5 ===')
try:
    g5 = json.load(open('scratch/eval_runs/g5-enterprise-portal.json', encoding='utf-8'))
    print('  status=%s pass_at=%s cost=$%.4f' % (g5['status'], g5.get('pass_at'), g5.get('cost',0)))
except Exception as e:
    print('  err', e)
