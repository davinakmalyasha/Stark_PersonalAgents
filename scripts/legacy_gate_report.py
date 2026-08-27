"""Detect-only sweep: which davinakmalyasha repos have gate workflows, and are
they HEALTHY or BROKEN (seeded by the pre-fix renderer)?

READ-ONLY by construction: only `gh api repos/...` GET calls and
`gh repo list`. Any non-GET argument would be a bug — asserted below.
Never writes, never patches, never deletes.

Run:
    .\\venv312\\Scripts\\python.exe scripts\\legacy_gate_report.py
"""
import base64
import json
import subprocess
import sys

OWNER = "davinakmalyasha"
TIERS = {
    "T1 fast-checks": ".github/workflows/fast-checks.yml",
    "T2 staging": ".github/workflows/staging.yml",
    "T3 release-heavy": ".github/workflows/release-heavy.yml",
}
LEVELS = ".harness-ci.json"


def gh(args, timeout=60):
    # GET-only guard: no method flags, no body flags, ever.
    assert not any(a in ("-X", "-f", "-F", "--input") for a in args), \
        f"legacy report must stay GET-only: {args}"
    r = subprocess.run(["gh", "api", *args], capture_output=True, text=True,
                       timeout=timeout)
    return r.returncode, (r.stdout or "") + (r.stderr or "")


def fetch(repo, path):
    rc, out = gh([f"repos/{OWNER}/{repo}/contents/{path}", "--jq", ".content"])
    if rc != 0 or out.strip() in ("", "null"):
        return None
    try:
        return base64.b64decode(out.replace("\n", "")).decode("utf-8")
    except Exception:
        return None


def t1_contract_ok(text, cfg):
    """T1 must: ignore prod levels + staging on push, keep PRs, no schedule."""
    import yaml
    try:
        data = yaml.safe_load(text)
    except Exception:
        return False, "YAML parse failure"
    on = data.get(True) or data.get("on") or {}
    push = on.get("push") or {}
    ig = set(push.get("branches-ignore") or [])
    prod = set((cfg.get("branches") or {}).get("prod") or []) | {"main", "master"}
    staging = (cfg.get("branches") or {}).get("staging") or "staging"
    if not prod <= ig:
        return False, f"branches-ignore missing prod {sorted(prod - ig)}"
    if staging and staging not in ig:
        return False, f"branches-ignore missing staging '{staging}'"
    if "pull_request" not in on or "workflow_dispatch" not in on:
        return False, f"trigger set wrong: {sorted(on)}"
    if "schedule" in on:
        return False, "schedule present"
    return True, "ok"


def classify(name):
    cfg_text = fetch(name, LEVELS)
    cfg = {}
    if cfg_text:
        try:
            cfg = json.loads(cfg_text)
        except Exception:
            cfg = {}
    tier_state = {}
    for label, path in TIERS.items():
        text = fetch(name, path)
        if text is None:
            tier_state[label] = ("MISSING", "")
            continue
        if label.startswith("T1"):
            ok, why = t1_contract_ok(text, cfg)
            tier_state[label] = ("OK" if ok else "BROKEN", why)
        else:
            import yaml
            try:
                yaml.safe_load(text)
                tier_state[label] = ("OK", "")
            except Exception as e:
                tier_state[label] = ("BROKEN", f"YAML: {str(e)[:80]}")
    states = {s for s, _ in tier_state.values()}
    if "MISSING" in states and len(states) == 1:
        verdict = "NOT-GATED"
    elif "MISSING" in states:
        verdict = "PARTIAL"
    elif "BROKEN" in states:
        verdict = "GATED-BROKEN (needs update_levels=true)"
    else:
        verdict = "GATED-OK"
    return verdict, tier_state


def main():
    rc, out = gh(["user", "--jq", ".login"])
    if rc != 0:
        print("gh not authenticated — run: gh auth login")
        return 1
    r = subprocess.run(["gh", "repo", "list", OWNER, "--limit", "100",
                        "--json", "name,isFork"],
                       capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        print(f"repo list failed: {r.stderr[:200]}")
        return 1
    repos = [x["name"] for x in json.loads(r.stdout) if not x.get("isFork")]
    print(f"Scanning {len(repos)} non-fork repos of {OWNER} (read-only)...\n")
    counts = {}
    rows = []
    for name in sorted(repos):
        verdict, tiers = classify(name)
        counts[verdict.split(" ")[0]] = counts.get(verdict.split(" ")[0], 0) + 1
        rows.append((name, verdict, tiers))
        detail = "; ".join(f"{k}={v}{' (' + w + ')' if w else ''}"
                           for k, (v, w) in tiers.items())
        print(f"{name:<32} {verdict}")
        print(f"  {detail}")
    print(f"\nSummary: {json.dumps(counts, indent=2)}")
    broken = [n for n, v, _ in rows if v.startswith("GATED-BROKEN")]
    if broken:
        print("\nRepos needing `github_apply_gate(repo, update_levels=True, gate='yes')`:")
        for n in broken:
            print(f"  - {OWNER}/{n}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
