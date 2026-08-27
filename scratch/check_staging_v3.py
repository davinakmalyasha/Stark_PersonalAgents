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
# 10 jobs
jobs=set(data.get("jobs",{}).keys())
check("10 jobs intact", jobs=={"backup-drill","n-minus-one","rotation-check","multi-worker","dns-preflight","tls-check","external-smoke","k6-load","staging-smoke","staging-summary"})
# no needs except summary
needs_jobs=[j for j,c in data["jobs"].items() if c.get("needs")]
check("only staging-summary has needs", needs_jobs==["staging-summary"])
# golden semantics
check("/ready 404 warn", "readiness probe NOT IMPLEMENTED" in rendered)
check("/ready 503 fatal", "DB genuinely down" in rendered)
check("backup md5 round-trip", "string_agg(v, ',' ORDER BY id)" in rendered and rendered.count("probe.md5")>=2)
check("n-1 head assert", "forward upgrade incomplete" in rendered)
check("no dead --workers fallback", "--workers 2" not in rendered.split("multi-worker:",1)[1].split("\n  dns-preflight:",1)[0])
check("gunicorn direct", 'gunicorn "$PKG.main:app" -w 2' in rendered)
# live guards
for j in ("dns-preflight","tls-check","external-smoke"):
    b=re.search(r"\n  "+j+r":\n(.*?)(?=\n  [a-z][a-z0-9-]*:\n|\Z)", rendered, re.DOTALL).group(1)
    check(f"{j} live guard", "vars.STAGING_URL != '' || github.event_name == 'workflow_dispatch'" in b and "STAGING_NO_LIVE" in b)
# per-job venv suffixes distinct
keys=re.findall(r"venv-fast-([a-z0-9-]+)-", rendered)
check(f"per-job venv suffixes distinct ({set(keys)})", len(set(keys))==5 and set(keys)=={"backup","n1","multiworker","k6","smoke"})
check("venv cache x5", rendered.count("path: ~/.venv-fast")==5)
check("k6 tarball cached", "key: k6-0.54.0-linux-amd64" in rendered)
# traceback assertions
check("multi-worker traceback assert", 'grep -q "Traceback" /tmp/gunicorn.log' in rendered)
check("k6 ephemeral traceback assert", 'grep -q "Traceback" /tmp/k6-server.log' in rendered)
check("smoke traceback assert", 'grep -q "Traceback" /tmp/smoke-server.log' in rendered)
# health_path/ready_path templated
check("health_path var in template", 'health_path' in t and 'ready_path' in t)
check("pg_version var in template", 'pg_version' in t)
check("postgres image templated x2", rendered.count("postgres:16-alpine")==2)
check("health_path used in smoke", rendered.count("/health")>=4)
check("ready_path used in smoke", "/ready" in rendered)
# rotation guard + shallower depth
check("rotation guard vault/schedule/dispatch + NO_ROTATION", "vars.STAGING_NO_ROTATION != 'true'" in rendered and "secrets.VAULT_ADDR != '' || github.event_name == 'schedule'" in rendered)
check("rotation fetch-depth 50", "fetch-depth: 50" in rendered)
# template.json vars
j=json.loads(pathlib.Path("D:/MyProject/LangChain/templates/config/github-staging/template.json").read_text(encoding="utf-8"))
vnames=[v["name"] for v in j["variables"]]
for v in ["health_path","ready_path","pg_version","k6_vus","k6_duration_s","k6_p95_ms","k6_error_rate_pct","tls_min_days"]:
    check(f"template.json var {v}", v in vnames)
check("notes mention per-job venv", "per-job" in j["notes"])
# hygiene
shas=re.findall(r"uses:\s*\S+@([0-9a-fA-F]+)", rendered)
check(f"{len(shas)} SHAs 40-hex", not [s for s in shas if len(s)!=40])
check("no | bash", "| bash" not in rendered)
check("no @latest", "@latest" not in rendered)
check("no jinja leak", not [l for l in rendered.splitlines() if ("{{" in l or "{%" in l) and "${{" not in l])
print("\nRESULT:", "ALL OK" if not fails else f"{len(fails)} FAILURES: {fails}"); sys.exit(1 if fails else 0)
