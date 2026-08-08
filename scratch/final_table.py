import json, glob, os, datetime

cats = ['gen2-bugfix','gen2-multifile','gen2-python','gen2-refactor','gen3-chain','gen3-log','gen3-long','gen3-trap','gen4-chain','gen4-log','gen4-long','gen4-trap']
now = datetime.datetime.now().timestamp()

# Fresh reports only (this full benchmark run; the re-run of g3l-async-await is also fresh)
def fresh(r):
    return now - os.path.getmtime(os.path.join('scratch/eval_runs', r['id'] + '.json')) < 7200

print("%-15s %6s %6s %6s %9s %8s %9s %9s" % ("category","tasks","pass","fail","cost$","dur_min","turns","t/task"))
tot = dict(n=0,p=0,f=0,cost=0.0,dur=0.0,turns=0)
for c in cats:
    n=p=f=0; cost=dur=turns=0.0
    for ff in glob.glob("scratch/eval_runs/*.json"):
        if "events" in ff: continue
        try: r=json.load(open(ff,encoding="utf-8"))
        except Exception: continue
        if r.get("category")!=c: continue
        if not fresh(r): continue
        n+=1
        if r.get("status")=="pass": p+=1
        else: f+=1
        cost+=r.get("cost",0) or 0; dur+=r.get("duration_s",0) or 0; turns+=r.get("paid_calls",0) or 0
    print("%-15s %6d %6d %6d %9.3f %8.1f %9.0f %9.1f" % (c,n,p,f,cost,dur/60,turns,turns/max(n,1)))
    tot['n']+=n; tot['p']+=p; tot['f']+=f; tot['cost']+=cost; tot['dur']+=dur; tot['turns']+=turns
print("-"*95)
print("%-15s %6d %6d %6d %9.3f %8.1f %9.0f %9.1f" % ("TOTAL",tot['n'],tot['p'],tot['f'],tot['cost'],tot['dur']/60,tot['turns'],tot['turns']/max(tot['n'],1)))
