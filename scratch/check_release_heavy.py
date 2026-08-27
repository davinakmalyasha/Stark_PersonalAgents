import pathlib, re, json, sys
from jinja2 import Template

p = pathlib.Path("D:/MyProject/LangChain/templates/config/github-release-heavy/files/release-heavy.yml.j2")
t = p.read_text(encoding="utf-8")
vars_ = dict(
    python_version="3.12", node_version="22", deno_version="v2.x", php_version="8.3",
    zizmor_version="1.29.0", zizmor_min_severity="low", trivy_version="0.73.0", trivy_severity="HIGH,CRITICAL",
    oasdiff_version="1.29.1", oasdiff_fail_on="ERR", cyclonedx_npm_version="6.0.1", cyclonedx_bom_version="7.3.1", cyclonedx_gomod_version="v1.7.0",
    job_timeout_minutes="15", license_denylist="GPL-3.0,AGPL-3.0",
    image_size_budget_mb="500", pg_version="16-alpine", health_path="/health", ready_path="/ready",
    perf_test_path="tests/test_perf_smoke.py", sbom_fail_on_empty="false", env_audit_strict="true"
)
rendered = Template(t).render(**vars_)
fails=[]
def check(name, cond):
    print(("OK  " if cond else "FAIL") + " " + name)
    if not cond: fails.append(name)

import yaml
try:
    data=yaml.safe_load(rendered)
    check("yaml parses", True)
except Exception as e:
    check("yaml parses "+str(e)[:200], False)
    data={}

jobs=data.get("jobs",{})
expected={"container-full","sbom-cyclonedx","license-compliance","env-audit","zizmor-full","debug-audit","schema-drift","migration-check","config-validation","endpoint-smoke","graceful-shutdown","api-contract-diff","perf-benchmark","build-full","static-asset-integrity","tests-detected","rollback-readiness","release-summary"}
check(f"{len(jobs)} jobs expected 18", set(jobs)==expected)

# triggers — dev-triggered only: push tags + dispatch, no schedule (removed per dev-control)
on = data.get(True) or data.get("on") or {}
check("triggers tags/dispatch (no schedule)", {"push","workflow_dispatch"} <= set(on.keys()) and "schedule" not in on)
# push tags v* and paths-ignore
push = on.get("push",{})
tags = push.get("tags",[])
check("push tags v*", "v*" in str(tags))
check("paths-ignore md/docs", "**.md" in str(push.get("paths-ignore")) and "docs/**" in str(push.get("paths-ignore")))
check("cancel-in-progress false", (data.get("concurrency") or {}).get("cancel-in-progress") is False)
check("no pull_request/merge_group on Tier3", "pull_request" not in on and "merge_group" not in on)

# SHAs
shas=re.findall(r"uses:\s*\S+@([0-9a-fA-F]+)", rendered)
check(f"{len(shas)} SHAs 40-hex", len(shas)>=10 and not [s for s in shas if len(s)!=40])

# live-API-verified pin allowlist (verified 2026-08 via api.github.com /tags)
ALLOWED_PINS = {
    "11d5960a326750d5838078e36cf38b85af677262",  # actions/checkout v4
    "704facf57e3136c30802ff274dee1b5dd6b55552",  # actions/cache v4
    "a26af69be951a213d495a4c3e4e4022e16d87065",  # actions/setup-python v5
    "49933ea5288caeca8642d1e84afbd3f7d6820020",  # actions/setup-node v4
    "44694675825211faa026b3c33019e3d171368870",  # actions/setup-go v5
    "c541c155eee45413f91358c6b0a30da1cbdc5c16",  # setup-php v2
    "5cca2a07f3dd05bdc1f6e1353728dbe5d2e604d4",  # setup-deno v2
    "043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",  # upload-artifact v7.0.1
    "3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",  # download-artifact v8.0.1
    "0e4a8c6effa4802afeda77dc8d303f8176d7dfad",  # dorny/paths-filter v3.0.4 (used by T1/T2; kept here as global reference)
}
bad_pins=[s for s in set(shas) if s not in ALLOWED_PINS]
check(f"all pins in verified allowlist {sorted(bad_pins)}", not bad_pins)

# no hashFiles() inside run: blocks (shell context — GHA expression would be invalid bash)
run_blocks=re.findall(r"run: \|\n((?:(?:[ ]{10,}|\t).*\n)+)", rendered)
hf_in_run=[b.splitlines()[0][:60] for b in run_blocks if "hashFiles(" in b]
check(f"no hashFiles inside run blocks ({len(run_blocks)} blocks scanned)", not hf_in_run)

# artifact upload steps must not carry a hashFiles-gated if (hashFiles pre-step is always pre-state)
up_steps=re.findall(r"actions/upload-artifact@\S+\n((?:\s+\S.*\n){0,8}?)\s*- (?:name|uses)|\Z", rendered)
check("no upload-artifact step gated by hashFiles if:", not re.search(r"upload-artifact@\S+\n(?:[ \t]+\S.*\n){0,6}[ \t]+if:.*hashFiles", rendered))

# registry-verified defaults (check SOURCE t — jinja consumes {{ }} at render)
check("oasdiff default 1.29.1", "oasdiff_version | default('1.29.1')" in t and "{{ oasdiff_version | default('1.29.1') }}/install.sh" in t)
check("cyclonedx-npm default 6.0.1", "@cyclonedx/cyclonedx-npm@{{ cyclonedx_npm_version | default('6.0.1') }}" in t)
check("cyclonedx-bom default 7.3.1", "cyclonedx-bom=={{ cyclonedx_bom_version | default('7.3.1') }}" in t)
check("oasdiff host canonical", "raw.githubusercontent.com/oasdiff/oasdiff/" in t)
check("checkout pinned 11d5960", "actions/checkout@11d5960a326750d5838078e36cf38b85af677262" in rendered)
check("setup-python pinned a26af69", "actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065" in rendered)
check("setup-node pinned 49933ea", "actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020" in rendered)
check("setup-go pinned 4469467", "actions/setup-go@44694675825211faa026b3c33019e3d171368870" in rendered)
check("setup-php pinned c541c15", "shivammathur/setup-php@c541c155eee45413f91358c6b0a30da1cbdc5c16" in rendered)
check("setup-deno pinned 5cca2a0", "denoland/setup-deno@5cca2a07f3dd05bdc1f6e1353728dbe5d2e604d4" in rendered)
check("cache pinned 704facf", "actions/cache@704facf57e3136c30802ff274dee1b5dd6b55552" in rendered)
check("no | bash / @latest", "| bash" not in rendered and "@latest" not in rendered)
check("no curl | sudo sh without cache", rendered.count("curl -fsSL https://raw.githubusercontent.com/tufin/oasdiff/main/install.sh")<=1) # we cache it

# caches — speed without quality loss
check("cache trivy", "key: trivy-" in rendered and "path: /tmp/trivy.tar.gz" in rendered)
check("cache zizmor", "key: zizmor-" in rendered and "path: /tmp/zizmor.tar.gz" in rendered)
check("cache oasdiff", "key: oasdiff-" in rendered and "path: /tmp/oasdiff.tar.gz" in rendered)
check("setup-python cache pip", rendered.count("cache: 'pip'")>=4)
check("setup-node cache npm", rendered.count("cache: 'npm'")>=2)
check("setup-go cache true", rendered.count("cache: true")>=2)

# universal SBOM
check("sbom py branch", "Generate SBOM — Python" in rendered)
check("sbom node branch", "Generate SBOM — Node" in rendered)
check("sbom go branch", "Generate SBOM — Go" in rendered and "go.mod" in rendered and "cyclonedx-gomod" in rendered)
check("sbom php branch", "Generate SBOM — PHP" in rendered and "composer.json" in rendered)
check("sbom deno branch", "Generate SBOM — Deno" in rendered and "deno.json" in rendered)
check("sbom merge step", "Merge SBOMs" in rendered and "sbom.json" in rendered and "polyglot" in rendered)
check("sbom setup-go", "actions/setup-go@4469" in rendered and "cyclonedx-gomod@v1.7.0" in rendered)
check("sbom setup-php", "shivammathur/setup-php@c541c1" in rendered and "Generate SBOM — PHP" in rendered)

# env-audit universal
check("env-audit go/php patterns", ("os.Getenv" in rendered or "os\\.Getenv" in rendered) and "getenv" in rendered and ("$_ENV" in rendered or "\\$_ENV" in rendered))
check("env-audit includes go/php files", "--include='*.go'" in rendered and "--include='*.php'" in rendered)

# endpoint-smoke adaptive
esh = jobs.get("endpoint-smoke",{})
check("endpoint-smoke adaptive if", "hashFiles('**/main.py') != '' || hashFiles('package.json') != '' || hashFiles('go.mod') != ''" in esh.get("if",""))
endpoint_block = re.search(r"\n  endpoint-smoke:\n(.*?)(?=\n  [a-z][a-z0-9-]*:\n|\Z)", rendered, re.DOTALL).group(1) if re.search(r"\n  endpoint-smoke:\n", rendered) else ""
check("endpoint python golden 200/404/503", "HEALTH_PATH" in endpoint_block and "READY_PATH" in endpoint_block and "200)" in endpoint_block and "404)" in endpoint_block and "503)" in endpoint_block)
check("endpoint node branch", "Node path" in rendered and 'npm start' in rendered)
check("endpoint go branch", "Go path" in rendered and "go run" in rendered)
check("endpoint php branch", "PHP path" in rendered and "php -S" in rendered)
check("endpoint setup-node conditional", "setup-node" in endpoint_block)
check("endpoint setup-go conditional", "setup-go" in endpoint_block)

# graceful-shutdown py-only gated
check("graceful-shutdown py gated", jobs["graceful-shutdown"].get("if","").strip() == "hashFiles('**/main.py') != ''")

# api-contract-diff adaptive
api = jobs.get("api-contract-diff",{})
check("api-contract-diff triggers any openapi", "hashFiles('openapi.json')" in api.get("if","") or "openapi.json" in api.get("if",""))
api_block = re.search(r"\n  api-contract-diff:\n(.*?)(?=\n  perf-benchmark:\n|\Z)", rendered, re.DOTALL).group(1) if re.search(r"\n  api-contract-diff:\n", rendered) else ""
check("api-contract-diff oasdiff cached", "Cache oasdiff" in api_block and "oasdiff" in api_block)
check("api-contract-diff fallback set-diff", "fallback set-diff" in rendered.lower() or "Fallback" in rendered)
check("api-contract-diff handles openapi.yaml", "openapi.yaml" in api_block)

# perf-benchmark universal
perf = jobs.get("perf-benchmark",{})
check("perf python+node+go", "hashFiles('tests/test_perf_smoke.py')" in perf.get("if","") and "hashFiles('tests/perf-smoke.mjs')" in perf.get("if","") and "hashFiles('tests/perf-smoke.go')" in perf.get("if",""))
perf_block = re.search(r"\n  perf-benchmark:\n(.*?)(?=\n  build-full:\n|\Z)", rendered, re.DOTALL).group(1) if re.search(r"\n  perf-benchmark:\n", rendered) else ""
check("perf go branch", "Perf benchmark (Go)" in perf_block and "go test" in perf_block)

# build-full / static-asset
check("build-full gated package.json", jobs["build-full"].get("if","").strip() == "hashFiles('package.json') != ''")
check("static-asset needs build-full", jobs["static-asset-integrity"].get("needs")== "build-full" or "build-full" in str(jobs["static-asset-integrity"].get("needs")))
check("static-asset gated package.json", jobs["static-asset-integrity"].get("if","").strip() == "hashFiles('package.json') != ''")

# tests-detected universal
tests_block = re.search(r"\n  tests-detected:\n(.*?)(?=\n  rollback-readiness:\n|\Z)", rendered, re.DOTALL).group(1) if re.search(r"\n  tests-detected:\n", rendered) else ""
check("tests-detected py branch", "python -m pytest" in tests_block)
check("tests-detected node branch", "npm test" in tests_block)
check("tests-detected go branch", "go test" in tests_block)
check("tests-detected deno branch", "deno test" in tests_block)
check("tests-detected php branch", "phpunit" in tests_block)
check("tests-detected setup-py/node/go/php/deno", tests_block.count("setup-python")>=1 and tests_block.count("setup-node")>=1 and tests_block.count("setup-go")>=1)

# rollback-readiness needs 3
rr = jobs.get("rollback-readiness",{})
needs = rr.get("needs",[])
check("rollback needs 3 (container+migration+sbom)", set(needs)=={"container-full","migration-check","sbom-cyclonedx"})
check("rollback sbom check", "SBOM_RES" in rendered and "sbom-cyclonedx" in rendered)
check("rollback digest check", "image-digest" in rendered and "digest" in rendered.lower())

# template.json universal checks
tj = json.loads(pathlib.Path("D:/MyProject/LangChain/templates/config/github-release-heavy/template.json").read_text(encoding="utf-8"))
check("template name github-release-heavy", tj["name"]=="github-release-heavy")
vnames=[v["name"] for v in tj["variables"]]
for v in ["python_version","node_version","deno_version","php_version","zizmor_version","zizmor_min_severity","trivy_version","trivy_severity","oasdiff_version","oasdiff_fail_on","cyclonedx_npm_version","cyclonedx_bom_version","cyclonedx_gomod_version","license_denylist","image_size_budget_mb","pg_version","health_path","ready_path","sbom_fail_on_empty","env_audit_strict"]:
    check(f"var {v}", v in vnames)
check("template notes universal", "single universal" in tj["notes"])

# shim templates use universal
for name in ["github-ci-security-python","github-ci-security-node","github-ci-security-generic"]:
    j=json.loads(pathlib.Path(f"D:/MyProject/LangChain/templates/config/{name}/template.json").read_text(encoding="utf-8"))
    check(f"{name} uses github-release-heavy", "github-release-heavy" in j.get("uses",[]))
    check(f"{name} no release-heavy file", not any("release-heavy" in f.get("src","") for f in j.get("files",[])))

# old files deleted
check("old python release-heavy deleted", not pathlib.Path("D:/MyProject/LangChain/templates/config/github-ci-security-python/files/release-heavy.yml.j2").exists())
check("old node release-heavy deleted", not pathlib.Path("D:/MyProject/LangChain/templates/config/github-ci-security-node/files/release-heavy.yml.j2").exists())
check("old generic release-heavy deleted", not pathlib.Path("D:/MyProject/LangChain/templates/config/github-ci-security-generic/files/release-heavy.yml.j2").exists())

# no jinja leak
check("no jinja leak", not [l for l in rendered.splitlines() if ("{{" in l or "{%" in l) and "${{" not in l])

# rollback: has checkout so Dockerfile guard is live
rr_block = re.search(r"\n  rollback-readiness:\n(.*?)\Z", rendered, re.DOTALL)
check("rollback has checkout (Dockerfile guard live)", bool(rr_block) and "actions/checkout@" in rr_block.group(1))
# dist artifact not hashFiles-gated; full digest emitted
check("build-full dist upload ungated", "name: dist-build" in rendered and "hashFiles('dist/**')" not in rendered)
check("full Id digest fallback", "d.get('Id','')" in rendered and "Id','')[:32]" not in rendered)
# no bogus DELETE DDL clause / PCRE-only dependency removed
check("destructive ERE clean", "DELETE\\s+FROM" not in rendered and "DROP[[:space:]]+(TABLE|COLUMN|INDEX|CONSTRAINT)" in rendered)
# deno.lock dual-form parser present
check("deno lock dual parser", "isinstance(pkgs, list)" in rendered and "lockfile v4" in rendered)
# oasdiff determinism: version= env passed to install.sh (script installs LATEST without it)
check("oasdiff version env pinned", "sudo version={{ oasdiff_version | default('1.29.1') }}" in t and "INSTALL_DIR=/usr/local/bin" in t)
# SBOM node: lockfile fast path, no npm ci on happy path, sbom node_modules cache removed
check("sbom package-lock-only fast path", "--package-lock-only" in t)
check("sbom node cache removed", "Cache node_modules (sbom)" not in t)
# endpoint: multi-port discovery
check("endpoint multi-port probe", "for P in 8000 3000 8080" in t and "wait_base()" in rendered)

print("\nRESULT:", "ALL OK" if not fails else f"{len(fails)} FAILURES: {fails}")
sys.exit(1 if fails else 0)
