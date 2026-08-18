"""swe_curate.py â€” git-log-driven discovery + validation of REAL bug-fix commits.

For each repo in the cache: scan history for fix commits that touch BOTH
source and tests, then validate each candidate the SWE-bench way:
  1. check out the parent commit (base) in a temp worktree
  2. apply the fix commit's TEST patch      -> hidden tests must FAIL
  3. apply the fix commit's SOURCE patch    -> hidden tests must PASS

Only validated entries are emitted to the manifest. Everything is
deterministic (pinned SHAs) and self-contained (no GitHub API).

Usage:
  venv312\\Scripts\\python.exe scratch\\swe_curate.py --repo <name> [--max-commits 1500] [--candidates 8] [--keep 3]
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys

CACHE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "swe_repo_cache")
WORKTREES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "swe_worktrees")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "swe_manifest_draft.json")


def _p(s):
    """Console-safe print (Windows cp1252 console chokes on some chars)."""
    try:
        print(s)
    except UnicodeEncodeError:
        print(s.encode("ascii", "replace").decode("ascii"))

_KEYWORDS = re.compile(r"(fix|bug|issue|error|broken|crash|regress|correct|handl|failing|wrong|invalid|fail)", re.I)
_MERGE_FIX_WORDS = re.compile(r"(fix|bug|issue|regress)", re.I)

TEST_SUFFIXES = (".test.js", ".test.mjs", ".test.ts", ".spec.js", ".spec.ts")
PY_TEST_DIRS = ("tests/", "test/")


def run(cmd, cwd=None, timeout=300, env=None):
    e = dict(os.environ)
    if env:
        e.update(env)
    try:
        # Windows: bare "npx"/"mocha" resolve to .cmd shims — subprocess can't
        # spawn those without shell=True, so normalize the executable name.
        if os.name == "nt" and cmd and "." not in cmd[0] and "/" not in cmd[0] and "\\" not in cmd[0]:
            resolved = shutil.which(cmd[0])
            if not resolved:
                # which() misses .cmd on some setups — probe explicitly
                for ex in (cmd[0] + ".cmd", cmd[0] + ".exe"):
                    if shutil.which(ex):
                        resolved = ex
                        break
            if resolved:
                cmd = [resolved] + list(cmd[1:])
        r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True,
                           timeout=timeout, env=e, errors="replace")
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return 124, "", f"TIMEOUT after {timeout}s"
    except Exception as ex:
        return -1, "", str(ex)


def is_test_file(rel: str) -> bool:
    low = rel.replace("\\", "/").lower()
    if low.endswith(TEST_SUFFIXES):
        return True
    name = low.rsplit("/", 1)[-1]
    if low.startswith(PY_TEST_DIRS) and (name.startswith("test_") or name.endswith("_test.py")):
        return True
    # Node repos like express/morgan put ALL tests in a top-level test/ dir
    # with plain names (test/res.send.js) — the directory is authoritative.
    if low.startswith("test/") and not low.startswith("test/support/"):
        return True
    return False


def test_runner_for(repo_dir: str, wt: str, base_sha: str) -> dict:
    """Detect the targeted-test command from the test script AT THE BASE COMMIT.

    The working-tree package.json (latest) may reference test helpers that did
    not exist at the base commit (e.g. yargs' test/before.mjs) — the runner
    must be derived from the base state. Also detects a required build step
    (tests running against build/ artifacts, e.g. yargs).
    """
    if os.path.isfile(os.path.join(repo_dir, "pyproject.toml")) or os.path.isfile(os.path.join(repo_dir, "setup.py")):
        return {"kind": "python", "venv": os.path.join(repo_dir, ".venv", "Scripts", "python.exe")}
    script = ""
    pj = {}
    rc, out, _ = run(["git", "-C", repo_dir, "show", f"{base_sha}:package.json"])
    if rc == 0:
        try:
            pj = json.loads(out)
            script = (pj.get("scripts") or {}).get("test", "") or ""
        except Exception:
            script = ""
    if not script:
        try:
            with open(os.path.join(repo_dir, "package.json"), encoding="utf-8") as f:
                pj = json.load(f)
                script = (pj.get("scripts") or {}).get("test", "") or ""
        except Exception:
            script = ""
    base_scripts = pj.get("scripts") or {}
    build = None
    if base_scripts.get("build") and not os.path.isdir(os.path.join(wt, "build")):
        # tests require build artifacts — must rebuild before running
        build = ["npm", "run", "build"]
    if "mocha" in script:
        if "test/before.mjs" in script or "before.mjs" in script:
            return {"kind": "node", "mocha": True, "build": build, "cmd": ["npx", "mocha", "__FILES__", "--require", "./test/before.mjs", "--timeout", "24000"]}
        if "test/support/env" in script:
            return {"kind": "node", "mocha": True, "build": build, "cmd": ["npx", "mocha", "__FILES__", "--require", "test/support/env"]}
        return {"kind": "node", "mocha": True, "build": build, "cmd": ["npx", "mocha", "__FILES__", "--timeout", "24000"]}
    if "tap" in script:
        return {"kind": "node", "tap": True, "build": build, "cmd": ["npx", "tap", "__FILES__", "--no-coverage", "--reporter=terse"]}
    if "tsx/esm" in script or "tsx" in script:
        return {"kind": "node", "build": build, "cmd": ["node", "--import", "tsx/esm", "--test", "__FILES__"]}
    if "node --test" in script:
        return {"kind": "node", "build": build, "cmd": ["node", "--test", "__FILES__"]}
    return {"kind": "node", "build": build, "cmd": ["node", "--test", "__FILES__"]}


def prepare_node_deps(repo_dir: str, base_sha: str) -> str | None:
    """Install the BASE COMMIT's dependency set into a shared per-commit dir.

    Returns the dir to junction as node_modules (e.g. express@HEAD dropped
    body-parser — old-commit tests need the deps as they were THEN). Reused by
    both validation and the eventual harness run. Returns None on failure.
    """
    sha8 = base_sha[:8]
    nm_dir = os.path.join(repo_dir, f"nm_{sha8}")
    if os.path.isdir(nm_dir) and os.path.isdir(os.path.join(nm_dir, "node_modules")):
        return nm_dir
    try:
        os.makedirs(nm_dir, exist_ok=True)
        rc, pj, _ = run(["git", "-C", repo_dir, "show", f"{base_sha}:package.json"])
        if rc != 0:
            return None
        with open(os.path.join(nm_dir, "package.json"), "w", encoding="utf-8") as f:
            f.write(pj)
        rc, lock, _ = run(["git", "-C", repo_dir, "show", f"{base_sha}:package-lock.json"])
        if rc == 0:
            with open(os.path.join(nm_dir, "package-lock.json"), "w", encoding="utf-8") as f:
                f.write(lock)
        e = dict(os.environ)
        e["npm_config_cache"] = os.path.join(os.path.dirname(repo_dir), "pkg_cache", "npm")
        rc, out, err = run(["npm", "install", "--no-audit", "--no-fund"], cwd=nm_dir, timeout=1800, env=e)
        if rc != 0:
            _p(f"      [deps-per-base] install failed: {err[-200:]}")
            return None
        _p(f"      [deps-per-base] installed nm_{sha8} ({len([d for d in os.listdir(os.path.join(nm_dir,'node_modules')) if os.path.isdir(os.path.join(nm_dir,'node_modules',d))])} pkgs)")
        return nm_dir
    except Exception as ex:
        _p(f"      [deps-per-base] error: {ex}")
        return None


def node_modules_junction(repo_dir: str, wt: str) -> bool:
    nm = os.path.join(repo_dir, "node_modules")
    if not os.path.isdir(nm):
        return False
    target = os.path.join(wt, "node_modules")
    if os.path.isdir(target):
        return True
    return run(["cmd", "/c", "mklink", "/J", target, nm])[0] == 0


def node_modules_junction_to(wt: str, nm_dir: str) -> bool:
    target = os.path.join(wt, "node_modules")
    if os.path.isdir(target):
        return True
    return run(["cmd", "/c", "mklink", "/J", target, nm_dir])[0] == 0


def apply_patch(wt: str, patch_text: str) -> bool:
    patch_path = os.path.join(wt, "_curate.patch")
    try:
        with open(patch_path, "w", encoding="utf-8") as f:
            f.write(patch_text)
    except Exception:
        return False
    rc, _, err = run(["git", "-C", wt, "apply", "--ignore-whitespace", "--whitespace=nowarn", patch_path])
    if rc != 0:
        print(f"      apply failed: {err.strip()[:200]}")
    try:
        os.remove(patch_path)
    except Exception:
        pass
    return rc == 0


def test_names_from_patch(patch_text: str) -> list[str]:
    """Extract test function/it names ADDED by the fix patch (scoped verdicts)."""
    names = []
    for line in patch_text.splitlines():
        if not line.startswith("+"):
            continue
        m = re.search(r"def test_(\w+)", line)
        if m:
            names.append("test_" + m.group(1))
            continue
        m = re.search(r"\b(it|test)\s*\(\s*['\"]([^'\"]{3,80})['\"]", line)
        if m:
            names.append(m.group(2))
    return names


def run_tests(runner: dict, wt: str, test_files: list[str], test_names: list[str]) -> tuple:
    """Run the targeted tests in the worktree. Returns (rc, output)."""
    if runner["kind"] == "python":
        py = runner["venv"]
        files = [os.path.join(wt, t) for t in test_files if os.path.isfile(os.path.join(wt, t))]
        if not files:
            return -1, "no test files present"
        # The venv has the package's DEPS but NOT the package itself (editable
        # install was removed â€” its import hook would shadow the worktree's
        # base code). PYTHONPATH supplies the WORKTREE checkout.
        pkg_root = os.path.join(wt, "src") if os.path.isdir(os.path.join(wt, "src")) else wt
        cmd = [py, "-m", "pytest", *files, "-q", "--no-header", "--timeout", "180"]
        if test_names:
            cmd += ["-k", " or ".join(test_names[:12])]
        rc, out, err = run(cmd, cwd=wt, timeout=600, env={"PYTHONPATH": pkg_root})
        return rc, out + "\n" + err
    # node
    files = [t for t in test_files
             if os.path.isfile(os.path.join(wt, t))
             and not t.endswith(".test-d.ts")   # tsd type tests — not runtime
             and not (runner.get("mocha") and t.endswith(".ts"))]
    if not files:
        return -1, "no test files present"
    if runner.get("build"):
        rc_b, out_b, err_b = run(runner["build"], cwd=wt, timeout=900)
        if rc_b != 0:
            return rc_b, f"build failed: {out_b[-500:]}\n{err_b[-500:]}"
    out_cmd = []
    for c in runner["cmd"]:
        if c == "__FILES__":
            out_cmd.extend(files)
        elif "__FILES__" in c:
            out_cmd.append(c.replace("__FILES__", " ".join(files)))
        else:
            out_cmd.append(c)
    cmd = out_cmd
    if test_names:
        pat = "|".join(re.escape(n) for n in test_names[:12])
        if runner.get("mocha"):
            cmd += ["--grep", pat]
        elif runner.get("tap"):
            cmd += ["--grep", pat]
        else:
            cmd += ["--test-name-pattern", pat]
    rc, out, err = run(cmd, cwd=wt, timeout=900)
    return rc, out + "\n" + err


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True)
    ap.add_argument("--max-commits", type=int, default=1500)
    ap.add_argument("--candidates", type=int, default=8)
    ap.add_argument("--keep", type=int, default=3)
    ap.add_argument("--since", default="2023-01-01",
                    help="only scan commits after this date (dep-compat window)")
    ap.add_argument("--deps-per-base", action="store_true",
                    help="install each base commit's OWN dependency set (node repos whose deps drift, e.g. express)")
    args = ap.parse_args()

    repo_dir = os.path.join(CACHE, args.repo)
    if not os.path.isdir(repo_dir):
        print(f"repo dir missing: {repo_dir}")
        sys.exit(1)
    runner = None  # detected per candidate (base-commit test script)
    os.makedirs(WORKTREES, exist_ok=True)

    rc, log, _ = run(["git", "-C", repo_dir, "log", "--no-merges", "--since", args.since,
                      "--format=%H|%s", "-n", str(args.max_commits)])
    commits = []
    for line in log.strip().splitlines():
        if "|" not in line:
            continue
        sha, subject = line.split("|", 1)
        commits.append((sha, subject))

    # Merge-based repos (express) squash real fixes INTO merge commits; include
    # them explicitly (diffed against first parent).
    rc, mlog, _ = run(["git", "-C", repo_dir, "log", "--merges", "--since", args.since,
                       "--format=%H|%s", "-n", str(args.max_commits)])
    for line in mlog.strip().splitlines():
        if "|" not in line:
            continue
        sha, subject = line.split("|", 1)
        commits.append((sha, subject, True))

    # Candidate scan
    candidates = []
    seen = set()
    for entry in commits:
        is_merge = len(entry) == 3
        sha, subject = entry[0], entry[1]
        if not _MERGE_FIX_WORDS.search(subject):
            continue
        if is_merge:
            base = f"{sha}^1"
        else:
            base = f"{sha}^"
        rc, files_out, _ = run(["git", "-C", repo_dir, "diff", "--name-only", base, sha])
        files = [f for f in files_out.strip().splitlines() if f.strip() and not f.startswith("docs/")]
        test_files = [f for f in files if is_test_file(f)]
        src_files = [f for f in files if not is_test_file(f) and not f.endswith((".lock", ".json", ".md"))]
        if not test_files or not src_files:
            continue
        key = tuple(sorted(src_files))[:3]
        if key in seen:
            continue
        seen.add(key)
        rc, numstat, _ = run(["git", "-C", repo_dir, "diff", "--numstat", base, sha])
        added = sum(int(l.split("\t")[0]) for l in numstat.splitlines() if l.split("\t")[0].isdigit())
        if added < 3:
            continue
        score = len(re.findall(r"(fix|bug|error|wrong|invalid|fail|crash|regress)", subject, re.I))
        if len(src_files) <= 5:
            score += 1
        candidates.append({"sha": sha, "subject": subject, "score": score, "merge": is_merge,
                           "src": src_files, "test": test_files, "added": added})

    candidates.sort(key=lambda c: (-c["score"], -c["added"]))
    print(f"[{args.repo}] {len(candidates)} candidates, validating top {args.candidates}...")

    valid = []
    for cand in candidates[:args.candidates]:
        sha, subject = cand["sha"], cand["subject"]
        base = f"{sha}^1" if cand.get("merge") else f"{sha}^"
        diff_spec = [base, sha]
        short = sha[:8]
        wt = os.path.join(WORKTREES, f"{args.repo}_{short}")
        if os.path.isdir(wt):
            shutil.rmtree(wt, ignore_errors=True)
        _p(f"  [{short}] {subject[:90]}")
        rc, _, err = run(["git", "-C", repo_dir, "worktree", "add", "--detach", wt, base])
        if rc != 0:
            print(f"      worktree add failed: {err.strip()[:150]}")
            continue
        try:
            runner = test_runner_for(repo_dir, wt, base)
            nm_hint = None
            if runner["kind"] == "node":
                if args.deps_per_base:
                    nm_hint = prepare_node_deps(repo_dir, base)
                    if nm_hint:
                        node_modules_junction_to(wt, os.path.join(nm_hint, "node_modules"))
                    else:
                        print("      [deps-per-base] failed — skip")
                        continue
                else:
                    node_modules_junction(repo_dir, wt)
            # 1. test patch only -> must FAIL
            rc, tp, _ = run(["git", "-C", repo_dir, "diff", *diff_spec, "--"] + cand["test"])
            if rc != 0 or not apply_patch(wt, tp):
                print("      test patch apply failed — skip")
                continue
            names = test_names_from_patch(tp)
            rc1, out1 = run_tests(runner, wt, cand["test"], names)
            # 2. + source patch -> must PASS
            rc, sp, _ = run(["git", "-C", repo_dir, "diff", *diff_spec, "--"] + cand["src"])
            if not apply_patch(wt, sp):
                print("      src patch apply failed — skip")
                continue
            rc2, out2 = run_tests(runner, wt, cand["test"], names)

            # Strict verdict: the bug reproduces only when the base run exits
            # non-zero. (Substring matching on "error/failed" is a trap — many
            # real tests PASS with those words in their names.)
            base_fails = rc1 != 0
            fix_passes = rc2 == 0
            status = "VALID" if (base_fails and fix_passes) else "reject"
            _p(f"      base rc={rc1} {'FAIL' if base_fails else 'pass?':5} | fix rc={rc2} {'PASS' if fix_passes else 'fail?':5} -> {status}")
            if rc1 == -1 or rc2 == -1:
                _p(f"      [-1 detail] {str(out1)[-200:]} | {str(out2)[-200:]}")
            if status == "VALID":
                valid.append({"repo": args.repo, "lang": runner["kind"], "base_sha": base,
                              "fix_sha": sha, "subject": subject, "src_files": cand["src"],
                              "test_files": cand["test"],
                              "nm_dir": os.path.basename(nm_hint) if nm_hint else None})
                if len(valid) >= args.keep:
                    break
        finally:
            # Junction MUST be removed before git removes the worktree: on
            # Windows both `git worktree remove` and `rmdir /s /q` FOLLOW the
            # junction and delete the node_modules TARGET (wipes the cache).
            # Plain `rmdir` removes only the link — safe either way.
            try:
                run(["cmd", "/c", "rmdir", os.path.join(wt, "node_modules")])
            except Exception:
                pass
            run(["git", "-C", repo_dir, "worktree", "remove", "--force", wt])

    manifest = []
    if os.path.isfile(OUT):
        with open(OUT, encoding="utf-8-sig") as f:
            manifest = json.load(f)
    manifest.extend(valid)
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)
    print(f"[{args.repo}] done: {len(valid)} new validated entries (total manifest {len(manifest)})")


if __name__ == "__main__":
    main()

