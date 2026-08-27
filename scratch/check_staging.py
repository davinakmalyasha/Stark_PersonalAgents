import pathlib, re, json, sys
from jinja2 import Template
import yaml

fpath = pathlib.Path(r"D:\MyProject\LangChain/templates/config/github-staging/files/staging.yml.j2")
f = fpath.read_text(encoding="utf-8")

# raw block coverage for GitHub expressions
raw_blocks = re.findall(r"\{% raw %\}(.*?)\{% endraw %\}", f, re.DOTALL)
raw_text = "\n".join(raw_blocks)
bad = []
for i, line in enumerate(f.split("\n"), 1):
    if "${{" in line and line.strip() not in raw_text:
        # check if line is inside raw by substring
        if not any("${{" in b and line.strip().split("${{")[0].strip()[:10] in b for b in raw_blocks):
            # simpler: is this line between a raw pair? just check count
            pass
        # fallback: count raw markers before line
        before = "\n".join(f.split("\n")[:i-1])
        open_raw = before.count("{% raw %}")
        close_raw = before.count("{% endraw %}")
        if open_raw <= close_raw:
            bad.append((i, line.strip()[:140]))
print("raw blocks", len(raw_blocks), "bad outside raw", bad[:10])
print("timeout lines", sum(1 for l in f.split("\n") if "timeout-minutes" in l))

rendered = Template(f).render(staging_branch="staging", staging_url="", python_version="3.12", node_version="22", job_timeout_minutes="20", gitleaks_version="8.30.1", k6_version="0.54.0", rotation_days="90")
data = yaml.safe_load(rendered)
print("yaml ok jobs", list(data["jobs"].keys()))
leftover_jinja = [l for l in rendered.split("\n") if "{{" in l or "{%" in l]
# filter allowed GitHub expr leftovers (they appear as ${{ not {{)
import re as re2
true_leak = [l for l in leftover_jinja if "${{" not in l]
print("true jinja leak", true_leak[:5])
for job, cfg in data["jobs"].items():
    needs = cfg.get("needs")
    if needs:
        print(job, "needs", needs)
# template.json
j = json.loads(pathlib.Path(r"D:\MyProject\LangChain/templates/config/github-staging/template.json").read_text(encoding="utf-8"))
print("template", j["name"], "vars", [v["name"] for v in j["variables"]])
j2 = json.loads(pathlib.Path(r"D:\MyProject\LangChain/templates/stack/stack-prod/template.json").read_text(encoding="utf-8"))
print("stack-prod uses", j2["uses"])
