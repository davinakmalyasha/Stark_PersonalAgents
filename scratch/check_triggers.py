import pathlib, yaml
from jinja2 import Template

ROOT = pathlib.Path("D:/MyProject/LangChain")
VARS = dict(staging_branch="staging", prod_branches="main,master", staging_url="",
            python_version="3.12", node_version="22", job_timeout_minutes="20",
            gitleaks_version="8.30.1", k6_version="0.54.0", rotation_days="90",
            trivy_version="0.73.0", zizmor_version="1.29.0",
            image_size_budget_mb="500", license_denylist="GPL-3.0",
            perf_test_path="tests/test_perf_smoke.py", python_version_generic="3.12")

def render(rel):
    t = (ROOT / rel).read_text(encoding="utf-8")
    data = yaml.safe_load(Template(t).render(**VARS))
    return data.get(True) or data.get("on") or {}

fails = []

# ── Tier 1: push on everything EXCEPT prod levels; PRs kept; no schedule ──
on = render("templates/config/github-fast-checks/files/fast-checks.yml.j2")
ig = on.get("push", {}).get("branches-ignore") or []
if not {"main", "master"}.issubset(ig):
    fails.append(f"T1 branches-ignore missing prod names: {ig}")
if "branches" in on.get("push", {}):
    fails.append("T1 must not restrict push to named branches")
if "pull_request" not in on or "workflow_dispatch" not in on or "schedule" in on:
    fails.append(f"T1 unexpected trigger set: {list(on)}")

# ── Tier 2: ONLY the staging-level branch; wd; no schedule, no pull_request ──
on = render("templates/config/github-staging/files/staging.yml.j2")
br = on.get("push", {}).get("branches") or []
if br != ["staging"]:
    fails.append(f"T2 push branches != ['staging']: {br}")
if "workflow_dispatch" not in on or "schedule" in on or "pull_request" in on:
    fails.append(f"T2 unexpected trigger set: {list(on)}")

# ── Tier 3: prod-level pushes + tags v*; wd; no schedule/pull_request ──
on = render("templates/config/github-release-heavy/files/release-heavy.yml.j2")
p3 = on.get("push", {})
if p3.get("tags") != ["v*"]:
    fails.append(f"T3 tags wrong: {p3.get('tags')}")
if not {"main", "master"}.issubset(p3.get("branches") or []):
    fails.append(f"T3 prod push branches missing: {p3.get('branches')}")
if "workflow_dispatch" not in on or "schedule" in on or "pull_request" in on:
    fails.append(f"T3 unexpected trigger set: {list(on)}")

# ── Shims ship ZERO workflow files (one file per tier only) ──
import json as _json
for prof in ("python", "node", "generic"):
    meta = _json.loads((ROOT / f"templates/config/github-ci-security-{prof}/template.json")
                       .read_text(encoding="utf-8"))
    wf = [f.get("dest") for f in meta.get("files", [])
          if str(f.get("dest", "")).startswith(".github/")]
    if wf:
        fails.append(f"shim {prof} ships workflow files: {wf}")

# tier headers still present
for rel in ("github-fast-checks", "github-staging", "github-release-heavy"):
    t = (ROOT / f"templates/config/{rel}/files").glob("*.yml.j2").__iter__().__next__().read_text(encoding="utf-8")
    if not any(k in t for k in ("TIER 1", "TIER 2", "TIER 3")):
        fails.append(f"{rel}: tier header lost")

print("T1 push:", ig)
print("T2 push:", br)
print("T3 push:", p3.get("branches"), "+", p3.get("tags"))
if fails:
    print("\nFAILURES:")
    [print(" -", f) for f in fails]
    raise SystemExit(1)
print("\nALL TRIGGER CHECKS PASSED")
