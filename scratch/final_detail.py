import json, glob, os, datetime

cats = ['gen2-bugfix','gen2-multifile','gen2-python','gen2-refactor','gen3-chain','gen3-log','gen3-long','gen3-trap','gen4-chain','gen4-log','gen4-long','gen4-trap']
now = datetime.datetime.now().timestamp()

def fresh(r):
    try:
        return now - os.path.getmtime(os.path.join('scratch/eval_runs', r['id'] + '.json')) < 14400
    except Exception:
        return False

print("%-15s %6s %5s %5s %8s %8s %12s %12s %10s %11s %10s %10s" % (
    "category", "tasks", "pass", "fail", "cost$", "dur_m", "turns", "input", "output", "reason", "c_hit", "c_miss"))
tot = dict(n=0,p=0,f=0,cost=0.0,dur=0.0,turns=0,inp=0,out=0,reason=0,hit=0,miss=0)
for c in cats:
    d = dict(n=0,p=0,f=0,cost=0.0,dur=0.0,turns=0,inp=0,out=0,reason=0,hit=0,miss=0)
    for ff in glob.glob("scratch/eval_runs/*.json"):
        if "events" in ff: continue
        try: r=json.load(open(ff,encoding="utf-8"))
        except Exception: continue
        if r.get("category")!=c: continue
        if not fresh(r): continue
        d['n']+=1
        if r.get("status")=="pass": d['p']+=1
        else: d['f']+=1
        d['cost']+=r.get("cost",0) or 0; d['dur']+=r.get("duration_s",0) or 0
        d['turns']+=r.get("paid_calls",0) or 0
        d['inp']+=r.get("input_tokens",0) or 0; d['out']+=r.get("output_tokens",0) or 0
        d['reason']+=r.get("reasoning_tokens",0) or 0
        d['hit']+=r.get("cache_hit",0) or 0; d['miss']+=r.get("cache_miss",0) or 0
    print("%-15s %6d %5d %5d %8.3f %8.1f %12d %12d %10d %11d %10d %10d" % (
        c, d['n'], d['p'], d['f'], d['cost'], d['dur']/60, d['turns'], d['inp'], d['out'], d['reason'], d['hit'], d['miss']))
    for k in tot: tot[k]+=d[k]
print("-"*130)
print("%-15s %6d %5d %5d %8.3f %8.1f %12d %12d %10d %11d %10d %10d" % (
    "TOTAL", tot['n'], tot['p'], tot['f'], tot['cost'], tot['dur']/60, tot['turns'], tot['inp'], tot['out'], tot['reason'], tot['hit'], tot['miss']))
print()
print("avg turns/task: %.1f | avg cost/task: $%.4f | reason/out: %.1f%% | cache-hit %% of input: %.1f%%" % (
    tot['turns']/max(tot['n'],1), tot['cost']/max(tot['n'],1), 100*tot['reason']/max(tot['out'],1), 100*tot['hit']/max(tot['inp'],1)))
