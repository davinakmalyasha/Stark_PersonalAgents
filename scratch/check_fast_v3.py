import pathlib, re, sys
from jinja2 import Template

t = pathlib.Path("D:/MyProject/LangChain/templates/config/github-fast-checks/files/fast-checks.yml.j2").read_text(encoding="utf-8")

vars_ = dict(python_version="3.12", node_version="22", gitleaks_version="8.30.1",
             actionlint_version="1.7.7", shellcheck_version="0.10.0", ruff_version="0.9.6",
             govulncheck_version="v1.1.4", deno_version="v2.x", job_timeout_minutes="5")
rendered = Template(t).render(**vars_)

fails = []
def check(name, cond):
    print(("OK  " if cond else "FAIL") + " " + name)
    if not cond:
        fails.append(name)

# yaml validity
import yaml
try:
    data = yaml.safe_load(rendered)
    check("yaml parses", True)
except Exception as e:
    check("yaml parses: " + str(e)[:200], False)
    data = {}

jobs = list(data.get("jobs", {}).keys())
check("5 jobs (inc fast-summary aggregator): " + str(jobs), jobs == ["secrets-workflow", "lint", "tests", "quick-audit", "fast-summary"])

# no cross-job needs except fast-summary aggregator (inline filtering + summary)
check("only fast-summary has needs", [j for j,c in data.get("jobs",{}).items() if c.get("needs")] == ["fast-summary"])

# dorny inlined 3x, pinned to verified commit, guarded to diff events only
check("dorny inline x3", rendered.count("dorny/paths-filter@0e4a8c6effa4802afeda77dc8d303f8176d7dfad") == 3)
check("filter steps guarded to push/PR x3", rendered.count("if: github.event_name == 'push' || github.event_name == 'pull_request'") == 3)
check("steps.filter.outputs referenced", "steps.filter.outputs.py" in rendered and "steps.filter.outputs.deno" in rendered)
check("no stale needs.changes refs", "needs.changes" not in rendered)

# single-source filter block: all three rendered blocks byte-identical
blocks = re.findall(r"filters: \|\n((?: {14}\S.*\n)+)", rendered)
identical = len(blocks) == 3 and len(set(blocks)) == 1
check(f"3 filter blocks byte-identical (got {len(blocks)})", identical)
check("macro source present once", t.count("{% set STACK_FILTERS -%}") == 1)

# pytest rc=5-only fallback (no double-run on real failures)
check("pytest rc5 gate", '= "5"' in rendered and "all tests deselected by 'not slow'" in rendered)
check("fallback suite appears once", rendered.count("pytest -x -q --tb=short") == 1)

# shellcheck integration
check("shellcheck cached+installed", "koalaman/shellcheck/releases/download/v0.10.0/shellcheck-v0.10.0.linux.x86_64.tar.xz" in rendered and "PATH=" in rendered.split("shellcheck-v")[1][:400])
check("actionlint mentions shellcheck", "auto-invokes shellcheck" in rendered)

# deno fmt guarded
check("deno fmt --check present+guarded", "deno fmt --check || exit 1" in rendered and "'\"fmt\"'" in rendered)

# merge_group full-run escape
check("merge_group full-run", "github.event_name == 'merge_group'" in rendered)

# consumer checkouts deep enough for push-event diffing
check("fetch-depth: 2 x3", rendered.count("fetch-depth: 2") == 3)

# venv cache (exact key)
check(".venv-fast cached", "~/.venv-fast" in rendered and "venv-fast-" in rendered)
check("venv exact-key only (no restore-keys on venv)", "restore-keys" not in rendered.split("venv-fast-")[1][:400])

# all uses SHAs exactly 40 hex
shas = re.findall(r"uses:\s*\S+@([0-9a-fA-F]+)", rendered)
bad = [s for s in shas if len(s) != 40]
check(f"{len(shas)} SHAs all 40-hex, bad={bad}", not bad and len(shas) >= 7)

# dorny pinned to verified commit (inline occurrences counted above)
check("dorny pinned SHA present", "dorny/paths-filter@0e4a8c6effa4802afeda77dc8d303f8176d7dfad" in rendered)

# no pipe-to-bash, no @latest installs
check("no '| bash'", "| bash" not in rendered)
check("no govulncheck@latest", "@latest" not in rendered)

# single-invocation tests (no head-pipe double-run), no -count=1
check("go test appears once", rendered.count("go test ./...") == 1)
check("deno test single invocation", rendered.count("deno test --allow-read") == 1)
check("no -count=1", "-count=1" not in rendered)

# changes gating wired into lint/tests/audit (inline)
for out in ("py", "node", "go", "php", "deno"):
    check(f"outputs.{out} referenced", f"steps.filter.outputs.{out}" in rendered)
check("workflow_dispatch full-run escape", "github.event_name == 'workflow_dispatch'" in rendered)

# node_modules cache (lint + tests; audit reads lockfile only) — count within full file
check("node_modules cache x2", rendered.count("path: node_modules") == 2)
check(".eslintcache cached", ".eslintcache" in rendered)
check("tsbuildinfo cached", "**/*.tsbuildinfo" in rendered)
check("ruff tarball cached", "key: ruff-0.9.6-x86_64-linux-gnu" in rendered)
check("gitleaks tarball cached", "key: gitleaks-8.30.1-linux_x64" in rendered)
check("actionlint+shellcheck tarballs cached (combined key)", "key: lint-tools-actionlint-1.7.7-shellcheck-0.10.0" in rendered)
check("govulncheck binary cached", "key: govulncheck-v1.1.4-go" in rendered)

# impact additions
check("prettier --check present", "prettier --check ." in rendered)
check("eslint --cache used", "--cache-location .eslintcache" in rendered)
check("tsc --incremental", "--noEmit --incremental" in rendered)
check("pytest -x -q -m not slow", 'pytest -x -q -m "not slow"' in rendered)
check("deno check via git ls-files", "git ls-files '*.ts'" in rendered)

# kill switches + tier header + no docker
check("FASTCHECK_NO_TESTS", "FASTCHECK_NO_TESTS" in rendered)
check("FASTCHECK_NO_AUDIT", "FASTCHECK_NO_AUDIT" in rendered)
check("TIER 1 header", "TIER 1" in rendered)
check("no docker/hadolint", "hadolint" not in rendered.lower() and "docker build" not in rendered)

# golden triggers intact
on = data.get("on") or data.get(True) or {}
trigs = list(on.keys()) if isinstance(on, dict) else []
check("triggers: " + str(trigs), set(["push", "pull_request", "merge_group", "workflow_dispatch"]) <= set(trigs))
conc = data.get("concurrency", {})
check("cancel-in-progress true", conc.get("cancel-in-progress") is True)

# template.json updated
j = __import__("json").loads(pathlib.Path("D:/MyProject/LangChain/templates/config/github-fast-checks/template.json").read_text(encoding="utf-8"))
vnames = [v["name"] for v in j["variables"]]
check("template vars incl ruff_version", "ruff_version" in vnames)
check("sample_vars incl ruff_version", "ruff_version" in j["sample_vars"])
check("notes mention diff-gating", "dorny/paths-filter" in j["notes"])

# no leftover jinja leak after render
leak = [l for l in rendered.splitlines() if ("{{" in l or "{%" in l) and "${{" not in l]
check("no jinja leak", not leak, )
if leak:
    print("   leak:", leak[:3])

print()
print("RESULT:", "ALL OK" if not fails else f"{len(fails)} FAILURES: {fails}")
sys.exit(1 if fails else 0)
