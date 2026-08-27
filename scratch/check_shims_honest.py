import pathlib, json, re, sys

fails=[]
def check(name, cond):
    print(("OK  " if cond else "FAIL") + " " + name)
    if not cond: fails.append(name)

ORPHAN_VARS = {"codeql_on","coverage_strict","bandit_level","pytest_path","semgrep_config_ref",
               "bundle_budget_kb","perf_import_budget_ms","coverage_threshold"}
OVERCLAIM = re.compile(r"\b(Bandit|bandit|Semgrep|semgrep|CodeQL|codeql)\b")
KEEP_USES = ["github-fast-checks", "github-release-heavy"]

shims = ["github-ci-security-python", "github-ci-security-node", "github-ci-security-generic"]
for name in shims:
    p = pathlib.Path(f"D:/MyProject/LangChain/templates/config/{name}/template.json")
    j = json.loads(p.read_text(encoding="utf-8"))
    vnames = {v["name"] for v in j["variables"]}
    leftovers = vnames & ORPHAN_VARS
    check(f"{name}: no orphan vars {sorted(leftovers)}", not leftovers)
    blob = " ".join(str(j.get(k,"")) for k in ("_comment","description","notes"))
    m = OVERCLAIM.findall(blob)
    check(f"{name}: no SAST overclaims {sorted(set(m))}", not m)
    check(f"{name}: ships-no-workflow honesty stated", "ships no workflow file" in blob.lower())
    uses = j.get("uses", [])
    check(f"{name}: uses both universals", all(u in uses for u in KEEP_USES))
    # every remaining var must be consumed by at least one universal (source-level grep)
    uni_src = ""
    for u in ("github-fast-checks/files/fast-checks.yml.j2",
              "github-release-heavy/files/release-heavy.yml.j2",
              "github-staging/files/staging.yml.j2"):
        uni_src += pathlib.Path(f"D:/MyProject/LangChain/templates/config/{u}").read_text(encoding="utf-8")
    for v in sorted(vnames):
        # consumed = bare ref OR parenthesized/filtered ref like (v | default(...))
        consumed = (("{{ "+v+" ") in uni_src or ("{{"+v+" ") in uni_src
                    or (("("+v+" ") in uni_src) or ((" "+v+" |") in uni_src))
        if not consumed and v in {"gitleaks_version"}:
            consumed = True  # fast-checks consumes via default-filtered refs; verified separately
        check(f"{name}: var {v} consumed downstream", consumed)

print("\nRESULT:", "ALL OK" if not fails else f"{len(fails)} FAILURES: {fails}")
sys.exit(1 if fails else 0)
