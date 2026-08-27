import pathlib, re, json, sys
from jinja2 import Template
t = pathlib.Path("D:/MyProject/LangChain/templates/config/github-staging/files/staging.yml.j2").read_text(encoding="utf-8")
vars_ = dict(staging_branch="staging", staging_url="", python_version="3.12", node_version="22",
             job_timeout_minutes="20", gitleaks_version="8.30.1", k6_version="0.54.0",
             k6_vus="10", k6_duration_s="30", k6_p95_ms="500", k6_error_rate_pct="1",
             tls_min_days="30", rotation_days="90", pg_version="16-alpine",
             health_path="/health", ready_path="/ready")
rendered = Template(t).render(**vars_)
fails=[]
def check(name, cond):
    print(("OK  " if cond else "FAIL") + " " + name)
    if not cond: fails.append(name)
import yaml
try: data=yaml.safe_load(rendered); check("yaml parses", True)
except Exception as e: check("yaml parses "+str(e)[:150], False); data={}
jobs=data.get("jobs",{})
check("10 jobs intact", set(jobs)=={"backup-drill","n-minus-one","rotation-check","multi-worker","dns-preflight","tls-check","external-smoke","k6-load","staging-smoke","staging-summary"})
check("only staging-summary has needs", [j for j,c in jobs.items() if c.get("needs")]==["staging-summary"])

# P1/P2 â€” live-capable gate compounds
smoke_if = jobs["staging-smoke"].get("if","")
k6_if = jobs["k6-load"].get("if","")
check("P1 smoke serves live-only repos", "hashFiles('**/main.py') != '' || vars.STAGING_URL != ''" in smoke_if)
check("P2 k6 compound (kill-switch + presence)", "vars.STAGING_NO_K6 != 'true'" in k6_if and "hashFiles('**/main.py') != '' || vars.STAGING_URL != ''" in k6_if)
check("multi-worker keeps local-app-only gate", jobs["multi-worker"].get("if","").strip() == "hashFiles('**/main.py') != ''")

# P4 â€” stale naming gone
check("no 'shared across staging jobs' remnants", "shared across staging jobs" not in rendered and "shared across staging jobs" not in t)
check("per-job step name x5", rendered.count("Cache venv (exact per-job key)")==5)

# P3 â€” kill-switch docs complete, _comment accurate
check("NO_ROTATION in header comment", "STAGING_NO_ROTATION" in t.split("{% set BACKEND_FILTERS")[0])
check("rotation guard present", "vars.STAGING_NO_ROTATION != 'true' && (secrets.VAULT_ADDR != '' || github.event_name == 'workflow_dispatch'" in rendered)
check("rotation depth 50", "fetch-depth: 50" in rendered)

j=json.loads(pathlib.Path("D:/MyProject/LangChain/templates/config/github-staging/template.json").read_text(encoding="utf-8"))
check("_comment lists exactly 8 ids", "#3 backup drill" in j["_comment"] and "#14 cosign / #15 canary belong to the deploy tier" in j["_comment"])
check("_comment no longer overclaims 14/15 as covered", "Maps to out-of-scope items #3,4,6,10,14,15,17,18,19,20" not in j["_comment"])
check("notes document NO_ROTATION", "STAGING_NO_ROTATION" in j["notes"])
check("description timing updated (~18s)", "~18s" in j["description"] and "(30s)" not in j["description"])
vnames=[v["name"] for v in j["variables"]]
for v in ["health_path","ready_path","pg_version","k6_vus","k6_duration_s","k6_p95_ms","k6_error_rate_pct","tls_min_days","rotation_days"]:
    check(f"var {v}", v in vnames)

# P5 â€” MODE accounts for dispatch input
check("MODE includes inputs.staging_url", "inputs.staging_url != ''" in rendered and "'STAGING_URL (live)'" in rendered)

# golden semantics regression block (from v3)
check("/ready 404 warn", "readiness probe NOT IMPLEMENTED" in rendered)
check("/ready 503 fatal", "DB genuinely down" in rendered)
check("backup md5 round-trip", "string_agg(v, ',' ORDER BY id)" in rendered and rendered.count("probe.md5")>=2)
check("n-1 head assert", "forward upgrade incomplete" in rendered)
check("backup PATH export", 'export PATH="$VENV/bin:$PATH"' in rendered.split("multi-worker:",1)[0].split("backup-drill:",1)[1])
mw = rendered.split("multi-worker:",1)[1].split("\n  dns-preflight:",1)[0]
check("traceback asserts x3 sites", all(s in rendered for s in ['/tmp/gunicorn.log','/tmp/k6-server.log','/tmp/smoke-server.log']))
check("gunicorn log captured", "> /tmp/gunicorn.log 2>&1 &" in mw)
keys=re.findall(r"venv-fast-([a-z0-9-]+)-", rendered)
check("5 distinct venv suffixes", len(set(keys))==5 and set(keys)=={"backup","n1","multiworker","k6","smoke"})
for jn in ("dns-preflight","tls-check","external-smoke"):
    b=re.search(r"\n  "+jn+r":\n(.*?)(?=\n  [a-z][a-z0-9-]*:\n|\Z)", rendered, re.DOTALL).group(1)
    check(f"{jn} live guard", "vars.STAGING_URL != '' || github.event_name == 'workflow_dispatch'" in b and "STAGING_NO_LIVE" in b)
check("dorny inline x5 pinned", rendered.count("dorny/paths-filter@0e4a8c6effa4802afeda77dc8d303f8176d7dfad")==5)
blocks=re.findall(r"filters: \|\n((?: {14}\S.*\n)+)", rendered)
check("filter blocks identical x5", len(blocks)==5 and len(set(blocks))==1)
check("full-run escape x10", rendered.count("'workflow_dispatch' || steps.filter.outputs.backend == 'true'")==10)
check("URL prologue x5 single-source", t.count("{% set URL_PROLOGUE -%}")==1 and rendered.count('URL="${INPUT_URL:-$STAGING_URL}"')==5)
shas=re.findall(r"uses:\s*\S+@([0-9a-fA-F]+)", rendered)
check(f"{len(shas)} SHAs 40-hex", not [s for s in shas if len(s)!=40] and len(shas)>=7)
check("no | bash / @latest", "| bash" not in rendered and "@latest" not in rendered)
on = data.get(True) or data.get("on") or {}
check("triggers push/dispatch (no schedule)", {"push","workflow_dispatch"} <= set(on.keys()) and "schedule" not in on)
check("cancel-in-progress false", (data.get("concurrency") or {}).get("cancel-in-progress") is False)
check("no jinja leak", not [l for l in rendered.splitlines() if ("{{" in l or "{%" in l) and "${{" not in l])

print("\nRESULT:", "ALL OK" if not fails else f"{len(fails)} FAILURES: {fails}"); sys.exit(1 if fails else 0)
