import pathlib, re, sys
from jinja2 import Template

t = pathlib.Path("D:/MyProject/LangChain/templates/config/github-staging/files/staging.yml.j2").read_text(encoding="utf-8")

vars_ = dict(staging_branch="staging", staging_url="", python_version="3.12", node_version="22",
             job_timeout_minutes="20", gitleaks_version="8.30.1", k6_version="0.54.0",
             k6_vus="10", k6_duration_s="30", k6_p95_ms="500", k6_error_rate_pct="1",
             tls_min_days="30", rotation_days="90")
rendered = Template(t).render(**vars_)

fails = []
def check(name, cond):
    print(("OK  " if cond else "FAIL") + " " + name)
    if not cond:
        fails.append(name)

import yaml
try:
    data = yaml.safe_load(rendered)
    check("yaml parses", True)
except Exception as e:
    check("yaml parses: " + str(e)[:200], False)
    data = {}

expected_jobs = {"backup-drill","n-minus-one","rotation-check","multi-worker","dns-preflight",
                 "tls-check","external-smoke","k6-load","staging-smoke","staging-summary"}
jobs = set(data.get("jobs", {}).keys())
check("10 jobs intact", jobs == expected_jobs)

# only summary has needs
needs_jobs = [j for j, c in data["jobs"].items() if c.get("needs")]
check("only staging-summary uses needs", needs_jobs == ["staging-summary"])

# golden semantics
check("/ready 404 warn", "readiness probe NOT IMPLEMENTED" in rendered and "404)" in rendered)
check("/ready 503 fatal (DB down)", "503" in rendered and "DB genuinely down" in rendered)
check("/ready 200 pass", "Readiness OK" in rendered)

# backup md5 round-trip
check("backup checksum written+verified", "string_agg(v, ',' ORDER BY id)" in rendered and rendered.count("probe.md5") >= 2)
check("data mismatch failure path", "DATA MISMATCH" in rendered)

# n-1 head assertion
check("n-1 asserts current==head", "alembic current" in rendered and "forward upgrade incomplete" in rendered)

# dead uvicorn fallback removed from multi-worker
mw = rendered.split("multi-worker:", 1)[1].split("\n  dns-preflight:", 1)[0]
check("no dead --workers fallback", "--workers 2" not in mw)
check("gunicorn direct launch", "gunicorn \"$PKG.main:app\" -w 2" in mw)

# live-only job guards (job level) on exactly dns/tls/external â€” regex block extraction
live_guard = "vars.STAGING_URL != '' || github.event_name == 'workflow_dispatch'"
def job_block(name):
    m = re.search(r"\n  " + name + r":\n(.*?)(?=\n  [a-z][a-z0-9-]*:\n|\Z)", rendered, re.DOTALL)
    return m.group(1) if m else ""
for j in ("dns-preflight", "tls-check", "external-smoke"):
    b = job_block(j)
    check(f"{j} job-level live guard", live_guard in b and "STAGING_NO_LIVE" in b)

# kill switches counted on code only (header comment mentions each once)
code_only = "\n".join(l for l in rendered.split("\n") if not l.lstrip().startswith("#"))
check("STAGING_NO_DB_DRILLS x2 (code)", code_only.count("vars.STAGING_NO_DB_DRILLS") == 2)
check("STAGING_NO_K6 x1 (code)", code_only.count("vars.STAGING_NO_K6") == 1)
check("STAGING_NO_LIVE x3 (code)", code_only.count("vars.STAGING_NO_LIVE") == 3)

# inline diff-gating: dorny x5, push/PR-guarded, byte-identical blocks
check("dorny inline x5", rendered.count("dorny/paths-filter@0e4a8c6effa4802afeda77dc8d303f8176d7dfad") == 5)
check("filter steps guarded to push/PR x5", rendered.count("if: github.event_name == 'push' || github.event_name == 'pull_request'") == 5)
blocks = re.findall(r"filters: \|\n((?: {14}\S.*\n)+)", rendered)
check(f"filter blocks identical x{len(blocks)}", len(blocks) == 5 and len(set(blocks)) == 5 - 4)
check("macro source once", t.count("{% set BACKEND_FILTERS -%}") == 1)
# gated steps: backup(2) + n1(2) + multi-worker(2) + k6(2) + smoke(2) = 10
check("full-run escape x10", rendered.count("'workflow_dispatch' || steps.filter.outputs.backend == 'true'") == 10)

# URL prologue macro single-source, rendered into 5 jobs
check("URL_PROLOGUE def once", t.count("{% set URL_PROLOGUE -%}") == 1)
check("URL prologue rendered x5", rendered.count('URL="${INPUT_URL:-$STAGING_URL}"') == 5)

# caches
check("venv cache x5", rendered.count("path: ~/.venv-fast") == 5)
check("venv exact-key (no restore-keys near venv)", "restore-keys" not in rendered.split("venv-fast-")[0][-100:] )
check("k6 tarball cached", f"key: k6-{vars_['k6_version']}-linux-amd64" in rendered)

# thresholds templated
check("k6 env thresholds wired", "__ENV.P95_MS" in rendered and "__ENV.ERR_PCT" in rendered and "__ENV.VUS" in rendered)
check("k6 vars injected", "VUS=10" in rendered and "P95_MS=500" in rendered and "ERR_PCT=1" in rendered)
check("tls budget templated", "TLS_MIN=30" in rendered)

# summary table
check("$GITHUB_STEP_SUMMARY table", "$GITHUB_STEP_SUMMARY" in rendered and "| job | result |" in rendered)
sm = rendered.split("staging-summary:", 1)[1]
for r in ["backup-drill","n-minus-one","rotation-check","multi-worker","dns-preflight","tls-check","external-smoke","k6-load","staging-smoke"]:
    if f"| {r} |" not in sm:
        check(f"summary row {r}", False)
        break
else:
    check("summary rows complete (9)", True)

# hygiene
shas = re.findall(r"uses:\s*\S+@([0-9a-fA-F]+)", rendered)
bad = [s for s in shas if len(s) != 40]
check(f"{len(shas)} SHAs all 40-hex, bad={bad}", not bad and len(shas) >= 7)
check("no '| bash'", "| bash" not in rendered)
check("no @latest", "@latest" not in rendered)

# triggers preserved (PyYAML parses bare `on` as boolean True)
on = data.get(True) or data.get("on") or {}
check("triggers push/schedule/dispatch", set(["push","schedule","workflow_dispatch"]) <= set(on.keys()))
check("pull_request still commented-out", "\n# pull_request:" in rendered or "# pull_request:" in t)
conc = data.get("concurrency", {})
check("cancel-in-progress false", conc.get("cancel-in-progress") is False)

# no jinja leak
leak = [l for l in rendered.splitlines() if ("{{" in l or "{%" in l) and "${{" not in l]
check("no jinja leak", not leak)

print()
print("RESULT:", "ALL OK" if not fails else f"{len(fails)} FAILURES: {fails}")
sys.exit(1 if fails else 0)
