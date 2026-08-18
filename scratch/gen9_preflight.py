"""gen9_preflight.py — end-to-end validation of the 18 gen9 tasks.

For each task: copy the frozen snapshot into a fresh workspace (exactly like
the eval runner), then
  1. run the hidden test                -> MUST FAIL (bug reproduces)
  2. apply the upstream fix (gold)      -> MUST PASS (task is solvable)

Every task must be green before the suite run is allowed.
"""
import json, os, shutil, subprocess, sys, tempfile
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = os.path.dirname(os.path.abspath(__file__))
CACHE = os.path.join(ROOT, "swe_repo_cache")
GEN9 = os.path.join(ROOT, "swe_gen9")
sys.path.insert(0, ROOT)
from gen9_real_tasks import run_hidden_test, _junction

tasks = json.load(open(os.path.join(GEN9, "tasks.json"), encoding="utf-8"))

def git(repo_dir, *args):
    r = subprocess.run(["git", "-C", repo_dir, *args], capture_output=True, text=True,
                       encoding="utf-8", errors="replace", timeout=120)
    return r.returncode == 0, r.stdout

ok_all = True
root = os.environ.get("BSA_EVAL_TMP", os.path.join(ROOT, "eval_tmp"))
os.makedirs(root, exist_ok=True)


def build_ws(t, tag):
    tmp = os.path.join(root, f"_preflight_{t['id']}_{tag}")
    shutil.rmtree(tmp, ignore_errors=True)
    os.makedirs(tmp, exist_ok=True)
    snap = os.path.join(GEN9, t["snapshot"])
    for entry in os.listdir(snap):
        src = os.path.join(snap, entry)
        dst = os.path.join(tmp, entry)
        if os.path.isdir(src):
            shutil.copytree(src, dst, ignore=shutil.ignore_patterns("node_modules", ".venv"))
        else:
            shutil.copy2(src, dst)
    if t["lang"] == "python":
        _junction(os.path.join(tmp, ".venv"), os.path.join(CACHE, t["repo"], ".venv"))
    else:
        nm = os.path.join(CACHE, t["repo"], (t.get("nm_dir") or "node_modules"), "node_modules") if t.get("nm_dir") else os.path.join(CACHE, t["repo"], "node_modules")
        _junction(os.path.join(tmp, "node_modules"), nm)
    return tmp

for t in tasks:
    if t.get("build_task"):
        # build tasks validated separately (acceptance vs reference) — skip here
        continue
    tid = t["id"]
    tmp = build_ws(t, "base")

    # 1. hidden test at base -> MUST FAIL
    fail_ok, detail = run_hidden_test(tmp, t)
    base_reproduces = not fail_ok and "TIMEOUT" not in detail
    shutil.rmtree(tmp, ignore_errors=True)

    # 2. fresh workspace + gold src patch -> MUST PASS
    pass_ok, pass_detail = False, "bug did NOT reproduce at base"
    if base_reproduces:
        tmp2 = build_ws(t, "gold")
        ok, gold = git(os.path.join(CACHE, t["repo"]), "diff", t["base_sha"], t["fix_sha"], "--", *t["src_files"])
        if ok and gold.strip():
            from gen9_real_tasks import _git_apply
            gold_ok, gold_err = _git_apply(tmp2, gold)
            if gold_ok:
                pass_ok, pass_detail = run_hidden_test(tmp2, t)
            else:
                pass_ok, pass_detail = False, "gold patch apply failed: " + gold_err[:200]
        else:
            pass_ok, pass_detail = False, "gold diff empty"
        shutil.rmtree(tmp2, ignore_errors=True)

    status = "GREEN" if (base_reproduces and pass_ok) else "RED"
    if status == "RED":
        ok_all = False
    print(f"[{status}] {tid:<28} base_fails={base_reproduces} gold_passes={pass_ok}")
    if status == "RED":
        print(f"        detail: {detail.strip()[:200]}")
        print(f"        gold:   {pass_detail.strip()[:200]}")

print("\n" + ("ALL 18 PREFLIGHT GREEN — suite ready" if ok_all else "PREFLIGHT FAILURES — fix before running"))
