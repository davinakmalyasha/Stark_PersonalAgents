import json, sys
from collections import Counter
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
p = r"D:\MyProject\LangChain\scratch\swe_manifest_draft.json"
m = json.load(open(p, encoding="utf-8-sig"))
DROP_PREFIX = {
    ("express", "69997cbd"),   # examples-only change, base passes (no real bug)
    ("express", "41113599"),   # refactor, base passes
}
m = [e for e in m if not any((e["repo"] == r and e["fix_sha"].startswith(s)) for r, s in DROP_PREFIX)]
seen = set(); out = []
for e in m:
    k = (e["repo"], e["fix_sha"])
    if k in seen: continue
    seen.add(k); out.append(e)
json.dump(out, open(p, "w", encoding="utf-8"), indent=2)
print("cleaned:", len(out), dict(Counter(e["repo"] for e in out)))
