"""LIVE end-to-end gate smoke test — the only phase that touches real GitHub.

Safety design (read before running):
  - Creates exactly ONE new PRIVATE repo named gate-smoke-<timestamp>.
  - Aborts if that name already exists (never reuses/overwrites anything).
  - Every API call addresses only that repo. No existing repo is referenced.
  - NEVER deletes anything (the token has no delete_repo scope anyway).
    At the end it prints the settings URL so a human can delete the repo.

Run:
    set LIVE_GATE_SMOKE=1
    .\\venv312\\Scripts\\python.exe scripts\\live_gate_smoke.py
"""
import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "tests"))

if os.environ.get("LIVE_GATE_SMOKE") != "1":
    print("Refusing to run: set LIVE_GATE_SMOKE=1 explicitly.")
    sys.exit(2)

# Windows consoles default to cp1252 — gate output contains arrows/emoji.
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

from _util import ci_gate, github_tools, temp_dir, write_file  # noqa: E402
import yaml  # noqa: E402

GT = github_tools()
CG = ci_gate()
OWNER = "davinakmalyasha"


def gh(args, timeout=60):
    r = subprocess.run(["gh", *args], capture_output=True, text=True, timeout=timeout)
    return r.returncode, (r.stdout or "") + (r.stderr or "")


def check(label, ok, detail=""):
    mark = "PASS" if ok else "FAIL"
    print(f"[{mark}] {label}" + (f" — {detail}" if detail else ""))
    return ok


def main():
    # 0. preflight
    rc, out = gh(["auth", "status"])
    if rc != 0:
        print("gh not authenticated — run: gh auth login")
        return 1
    ts = int(time.time())
    resume_repo = os.environ.get("LIVE_GATE_SMOKE_REPO", "").strip()
    if resume_repo:
        repo = resume_repo
        name = repo.split("/")[-1]
        print(f"\n== Resuming against EXISTING repo {repo} (no create) ==")
    else:
        name = f"gate-smoke-{ts}"
        repo = f"{OWNER}/{name}"
        rc, _ = gh(["repo", "view", repo])
        if rc == 0:
            print(f"ABORT: {repo} already exists — refusing to reuse an existing repo.")
            return 1

    results = []
    # 1. create + apply the gate (create=True, private)
    if resume_repo:
        print(f"\n== Re-affirming gate state on {repo} ==")
        out = GT.github_apply_gate(repo, stack="python", gate="yes")
        print(out[:2000])
        results.append(check("apply_gate reported success", "Error" not in out[:200]))
        results.append(check("seed message", "Seeded" in out))
        results.append(check("ladder message", "Branch ladder" in out))
    else:
        print(f"\n== Creating private repo {repo} and applying the gate ==")
        out = GT.github_apply_gate(name, stack="python", gate="yes",
                                   create=True, private=True,
                                   description="throwaway gate smoke test — safe to delete")
        print(out[:2000])
        results.append(check("apply_gate reported success", "Error" not in out[:200]))
        results.append(check("seed message", "Seeded" in out))
        results.append(check("ladder message", "Branch ladder" in out))
        results.append(check("default switched", "Default branch switched" in out))
    results.append(check("protection applied", "Protected main" in out))

    # 2. update_levels=True re-seed FIRST (the legacy-repo repair path) so the
    #    push test below runs against freshly rendered, repaired workflows.
    print(f"\n== update_levels=True re-seed (legacy repair path) ==")
    out = GT.github_apply_gate(repo, stack="python", gate="yes", update_levels=True)
    print(out[:1200])
    results.append(check("update_levels re-seeded level-owned files",
                         "added" in out or "Seeded" in out))

    # 3. remote state assertions on the repaired repo
    print(f"\n== Remote state assertions on {repo} ==")
    rc, out = gh(["api", f"repos/{repo}", "--jq", ".default_branch"])
    results.append(check("default branch == development", out.strip() == "development",
                         out.strip()))
    rc, out = gh(["api", f"repos/{repo}/branches?per_page=100", "--jq",
                  "[.[].name] | join(\",\")"])
    heads = {h.strip() for h in out.split(",") if h.strip()}
    results.append(check("ladder exists", {"development", "staging", "main"} <= heads,
                         str(sorted(heads))))
    rc, out = gh(["api", f"repos/{repo}/branches/main/protection",
                  "--jq", ".required_status_checks.contexts | join(\",\")"])
    remote_ctx = [c for c in out.strip().split(",") if c and c != "null"]
    t1_ids = GT._workflow_job_ids(repo, ".github/workflows/fast-checks.yml", ref="main")
    results.append(check("protection contexts == parsed T1 ids",
                         bool(remote_ctx) and remote_ctx == t1_ids,
                         f"{remote_ctx}"))
    for p in (".github/workflows/fast-checks.yml", ".github/workflows/staging.yml",
              ".github/workflows/release-heavy.yml", ".harness-ci.json"):
        rc, out = gh(["api", f"repos/{repo}/contents/{p}", "--jq", ".content"])
        ok = rc == 0 and out.strip() not in ("", "null")
        if ok:
            import base64
            text = base64.b64decode(out.replace("\n", "")).decode("utf-8")
            try:
                yaml.safe_load(text)
            except Exception as e:
                ok = False
                text = f"YAML parse failed: {e}"
        results.append(check(f"remote {p} fetch+parse", ok,
                             "" if ok else text[:120]))

    # 2b. every SHA-pinned action in the seeded workflows must RESOLVE upstream
    # (live-only check: fabricated pins pass every offline contract)
    import re
    import base64
    pins = set()
    for p in (".github/workflows/fast-checks.yml", ".github/workflows/staging.yml",
              ".github/workflows/release-heavy.yml"):
        rc, out = gh(["api", f"repos/{repo}/contents/{p}", "--jq", ".content"])
        if rc != 0:
            continue
        text = base64.b64decode(out.replace("\n", "")).decode("utf-8")
        for m in re.finditer(r"uses:\s*([\w.-]+/[\w.-]+)@([0-9a-f]{40})", text):
            pins.add((m.group(1), m.group(2)))
    bad_pins = []
    for arepo, sha in sorted(pins):
        rc, _ = gh(["api", f"repos/{arepo}/commits/{sha}", "--jq", ".sha"])
        if rc != 0:
            bad_pins.append(f"{arepo}@{sha[:12]}")
    results.append(check("all action pins resolve upstream",
                         not bad_pins, ", ".join(bad_pins[:6])))

    # 4. harness push → T1 fires → watch it green
    print(f"\n== Harness push simulation (ci_gate.push_if_changed) ==")
    with temp_dir(prefix="gatesmoke-") as tmp:
        clone = os.path.join(tmp, "clone")
        rc, out = gh(["repo", "clone", repo, clone, "--", "--quiet"])
        if rc != 0:
            print(f"clone failed: {out[:300]}")
            results.append(check("clone", False))
        else:
            write_file(os.path.join(clone, "smoke.txt"),
                       f"gate smoke {ts}\n")
            pushed = CG.push_if_changed(clone, "gate smoke: harness push")
            print({k: v for k, v in pushed.items() if k != "preview"})
            results.append(check("push landed on development",
                                 pushed.get("pushed") is True
                                 and pushed.get("branch") == "development",
                                 pushed.get("note", "")))
            if pushed.get("pushed"):
                sha = pushed["sha"]
                run_id = None
                deadline = time.time() + 120
                while time.time() < deadline and not run_id:
                    rc, out = gh(["api",
                                  f"repos/{repo}/actions/runs?head_sha={sha}&per_page=1",
                                  "--jq", ".workflow_runs[0].id"])
                    rid = out.strip()
                    if rc == 0 and rid and rid != "null":
                        run_id = rid
                    else:
                        time.sleep(5)
                results.append(check("T1 run appeared for pushed sha", run_id is not None))
                if run_id:
                    conclusion = None
                    deadline = time.time() + 900  # 15 min bound
                    while time.time() < deadline:
                        rc, out = gh(["api", f"repos/{repo}/actions/runs/{run_id}",
                                      "--jq", "[.status, .conclusion] | @tsv"])
                        parts = out.strip().split("\t")
                        if parts and parts[0] == "completed":
                            conclusion = parts[1] if len(parts) > 1 else "?"
                            break
                        time.sleep(15)
                    results.append(check("T1 run green", conclusion == "success",
                                         f"conclusion={conclusion} run={run_id}"))

    # summary
    passed = sum(results)
    print(f"\n{'='*60}\nLIVE SMOKE: {passed}/{len(results)} checks passed")
    print(f"Throwaway repo (delete manually): "
          f"https://github.com/{repo}/settings — scroll to Danger Zone.")
    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
