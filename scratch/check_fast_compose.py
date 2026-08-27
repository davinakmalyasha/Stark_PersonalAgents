import pathlib, json, sys
sys.path.insert(0, "D:/MyProject/LangChain")
from jinja2 import Template

# simulate templateTools apply for python profile with uses
# read python template.json
py = json.loads(pathlib.Path("D:/MyProject/LangChain/templates/config/github-ci-security-python/template.json").read_text(encoding="utf-8"))
print("python uses", py.get("uses"))
print("python files", [f["src"] for f in py["files"]])
# check that fast-checks not in files but via uses
assert "github-fast-checks" in py.get("uses",[])
assert all("fast-checks" not in f["src"] for f in py["files"])
print("python compose OK")

for name in ["github-ci-security-python","github-ci-security-node","github-ci-security-generic"]:
    j = json.loads(pathlib.Path(f"D:/MyProject/LangChain/templates/config/{name}/template.json").read_text(encoding="utf-8"))
    print(name, "uses", j.get("uses"), "fast in files", any("fast-checks" in f["src"] for f in j["files"]))

# check universal file renders
t = pathlib.Path("D:/MyProject/LangChain/templates/config/github-fast-checks/files/fast-checks.yml.j2").read_text(encoding="utf-8")
rendered = Template(t).render(python_version="3.12", gitleaks_version="8.30.1", actionlint_version="1.7.7", govulncheck_version="v1.1.4", deno_version="v2.x", node_version="22", job_timeout_minutes="5")
import re
jobs = re.findall(r'^\s{2}([a-z\-]+):\n\s+name:', rendered, re.M)
print("universal jobs", jobs)
assert all(x in jobs for x in ["secrets-workflow","lint","tests","quick-audit"])
# check hashFiles for each stack
for marker in ["'**/*.py'","package.json","go.mod","composer.json","deno.json"]:
    print(marker, marker in rendered)
# check no 39-char SHA
shas = re.findall(r'@([0-9a-f]+)', rendered)
bad = [s for s in shas if len(s)!=40]
print("bad shas", bad)
assert not bad, f"bad shas {bad}"
print("all checks OK")
# check tier headers
assert "TIER 1" in rendered
print("tier OK")
# check no docker
assert "hadolint" not in rendered.lower()
print("no docker OK")
# check kill switches
assert "FASTCHECK_NO_AUDIT" in rendered
assert "FASTCHECK_NO_TESTS" in rendered
print("kill switches OK")
