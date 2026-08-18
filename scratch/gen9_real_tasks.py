"""gen9_real_tasks.py — 18-task "gen9-real" suite: SWE-bench-style real bugs.

Each task pulls a REAL bug from a real open-source repo (pinned base commit),
with the upstream fix commit's own tests as the hidden test suite. The agent
works on a frozen snapshot of the repo at the base commit; verification runs
the hidden test patch in the repo's real environment (per-repo venv for
Python, junctioned node_modules for Node).

Tasks are defined in scratch/swe_gen9/tasks.json (built by gen9_build.py).
"""
import json
import os
import subprocess
import sys

_ROOT = os.path.dirname(os.path.abspath(__file__))
_CACHE = os.path.join(_ROOT, "swe_repo_cache")
_GEN9 = os.path.join(_ROOT, "swe_gen9")


def _load_tasks():
    with open(os.path.join(_GEN9, "tasks.json"), encoding="utf-8") as f:
        return json.load(f)


def _git_apply(d, patch_text):
    # git apply silently no-ops (rc=0, nothing applied) when an ENCLOSING repo
    # exists above d but d itself has no .git — e.g. the harness repo. Mirror
    # eval_runner's _git_init so apply always targets the workspace itself.
    if not os.path.isdir(os.path.join(d, ".git")):
        subprocess.run(["git", "init", "-q", d], capture_output=True, timeout=60)
        subprocess.run(["git", "-C", d, "config", "user.email", "harness@gen9"],
                       capture_output=True, timeout=60)
        subprocess.run(["git", "-C", d, "config", "user.name", "gen9"],
                       capture_output=True, timeout=60)
    patch_path = os.path.join(d, "_hidden_test.patch")
    with open(patch_path, "w", encoding="utf-8") as f:
        f.write(patch_text)
    r = subprocess.run(["git", "-C", d, "apply", "--ignore-whitespace", "--whitespace=nowarn", patch_path],
                       capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120)
    try:
        os.remove(patch_path)
    except Exception:
        pass
    return r.returncode == 0, (r.stderr or r.stdout)[-400:]


def _junction(link, target):
    link = os.path.abspath(link)
    target = os.path.abspath(target)
    subprocess.run(["cmd", "/c", "mklink", "/J", link, target],
                   capture_output=True, text=True, timeout=60)
    return os.path.isdir(link)


def run_hidden_test(d, task, timeout_s=600):
    """Run the hidden tests for a gen9 task inside workspace d.

    Returns (passed: bool, detail: str).
    """
    try:
        with open(os.path.join(_GEN9, task["testpatch"]), encoding="utf-8") as f:
            patch = f.read()
        ok, err = _git_apply(d, patch)
        if not ok:
            return False, f"hidden-test patch apply failed: {err}"

        repo_dir = os.path.join(_CACHE, task["repo"])
        test_files = task["test_files"]

        if task["lang"] == "python":
            py = os.path.join(repo_dir, ".venv", "Scripts", "python.exe")
            files = [os.path.join(d, t) for t in test_files if os.path.isfile(os.path.join(d, t))]
            if not files:
                return False, "no test files present"
            pkg_root = os.path.join(d, "src") if os.path.isdir(os.path.join(d, "src")) else d
            env = dict(os.environ)
            env["PYTHONPATH"] = pkg_root
            cmd = [py, "-m", "pytest", *files, "-q", "--no-header", "--timeout", "180"]
            # scope to the tests added by the hidden patch (whole-file runs pick
            # up unrelated pre-existing failures in these repos)
            import re as _re
            names = _re.findall(r"^\+\s*(?:async\s+)?def (test_\w+)", patch, flags=_re.M)
            if names:
                cmd += ["-k", " or ".join(dict.fromkeys(names))[:400]]
        else:
            # junction the repo's dependency set (per-base nm dir when present)
            nm = os.path.join(repo_dir, task["nm_dir"] or "node_modules", "node_modules") if task.get("nm_dir") else os.path.join(repo_dir, "node_modules")
            if not _junction(os.path.join(d, "node_modules"), nm):
                return False, "node_modules junction failed"
            from swe_curate import test_runner_for, test_names_from_patch, run
            runner = test_runner_for(repo_dir, d, task["base_sha"])
            if runner.get("build"):
                rc, out, err = run(runner["build"], cwd=d, timeout=900)
                if rc != 0:
                    return False, f"build failed: {(out + err)[-400:]}"
            names = test_names_from_patch(patch)
            files = [t for t in test_files
                     if os.path.isfile(os.path.join(d, t))
                     and not t.endswith(".test-d.ts")
                     and not (runner.get("mocha") and t.endswith(".ts"))]
            if not files:
                return False, "no test files present"
            cmd = []
            for c in runner["cmd"]:
                if c == "__FILES__":
                    cmd.extend(files)
                elif "__FILES__" in c:
                    cmd.append(c.replace("__FILES__", " ".join(files)))
                else:
                    cmd.append(c)
            if names:
                pat = "|".join(__import__("re").escape(n) for n in names[:12])
                if runner.get("mocha") or runner.get("tap"):
                    cmd += ["--grep", pat]
                else:
                    cmd += ["--test-name-pattern", pat]
            rc, out, err = run(cmd, cwd=d, timeout=timeout_s)
            if rc == 0:
                return True, "rc=0 PASS\n" + out[-600:]
            return False, f"rc={rc}\n{(out + err)[-1400:]}"

        r = subprocess.run(cmd, cwd=d, capture_output=True, text=True, encoding="utf-8", errors="replace",
                           timeout=timeout_s, env=env)
        if r.returncode == 0:
            return True, "rc=0 PASS\n" + r.stdout[-600:]
        return False, f"rc={r.returncode}\n{(r.stdout + r.stderr)[-1400:]}"
    except subprocess.TimeoutExpired:
        return False, f"TIMEOUT after {timeout_s}s"
    except Exception as ex:
        return False, f"hidden-test error: {ex}"


def _build_task(t):
    """Build-task dict: empty workspace + acceptance-based hidden test."""
    import subprocess as _sp
    venv_py = os.path.abspath(os.path.join(_ROOT, "..", "venv312", "Scripts", "python.exe"))
    accept = os.path.join(_GEN9, "acceptance", "accept_a.py" if t["id"] == "g9r-saas-platform" else "accept_b.py")
    tid = t["id"]

    def setup(d):
        try:
            from sync.state_sync import shared_state
            if t.get("budget", {}).get("remaining_steps"):
                shared_state["remaining_steps"] = t["budget"]["remaining_steps"]
            if t.get("budget", {}).get("max_iters"):
                os.environ["BSA_DEV_MAX_ITERS"] = str(t["budget"]["max_iters"])
        except Exception:
            pass
        if t["lang"] == "python":
            # junction the harness venv so the agent can run its own app/tests
            _junction(os.path.join(d, ".venv"), os.path.dirname(os.path.dirname(venv_py)))
        return t["prompt"]

    def test(d):
        r = _sp.run([venv_py, "-X", "utf8", accept, d],
                    capture_output=True, text=True, encoding="utf-8", errors="replace",
                    timeout=900)
        if r.returncode == 0:
            return "rc=0 PASS\n" + (r.stdout or "")[-300:]
        return f"STATUS: FAIL (rc={r.returncode})\n{(r.stdout or '')[-800:]}{(r.stderr or '')[-500:]}"

    return {
        "id": tid,
        "category": t["category"],
        "name": t["name"],
        "prompt": t["prompt"],
        "setup": setup,
        "test": test,
        "budget": t["budget"],
        "lang": t["lang"],
        "repo": t["repo"],
        "gen9_task": t,
    }


def build_tasks(H):
    """Build the gen9 task dicts for eval_runner."""
    tasks = []
    for t in _load_tasks():
        if t.get("build_task"):
            tasks.append(_build_task(t))
            continue
        snap = os.path.join(_GEN9, t["snapshot"])
        tid = t["id"]

        def make_setup(snap_dir, lang, repo, nm_dir, base_sha, prompt, budget):
            def setup(d):
                import shutil
                for entry in os.listdir(snap_dir):
                    src = os.path.join(snap_dir, entry)
                    dst = os.path.join(d, entry)
                    if os.path.isdir(src):
                        shutil.copytree(src, dst, ignore=shutil.ignore_patterns("node_modules", ".venv"))
                    else:
                        shutil.copy2(src, dst)
                # per-task budget override (harness state + env)
                try:
                    from sync.state_sync import shared_state
                    if budget and budget.get("remaining_steps"):
                        shared_state["remaining_steps"] = budget["remaining_steps"]
                    if budget and budget.get("max_iters"):
                        os.environ["BSA_DEV_MAX_ITERS"] = str(budget["max_iters"])
                except Exception:
                    pass
                # environment hint (SWE-bench style) so the agent can self-verify
                if lang == "python":
                    venv = os.path.join(_CACHE, repo, ".venv")
                    if os.path.isdir(venv):
                        _junction(os.path.join(d, ".venv"), venv)
                else:
                    nm = os.path.join(_CACHE, repo, nm_dir or "node_modules", "node_modules") if nm_dir else os.path.join(_CACHE, repo, "node_modules")
                    if os.path.isdir(nm):
                        _junction(os.path.join(d, "node_modules"), nm)
                hint = ("\n\n(environment: a ready-made venv with all dependencies is at `.venv` — run "
                        "tests with `.venv/Scripts/python -m pytest`)" if lang == "python" else
                        "\n\n(environment: dependencies are pre-installed — `npm test` / `npx mocha` work)")
                return prompt + hint
            return setup

        tasks.append({
            "id": tid,
            "category": t["category"],
            "name": t["repo"],
            "prompt": t["prompt"],
            "setup": make_setup(snap, t["lang"], t["repo"], t.get("nm_dir"), t["base_sha"], t["prompt"], t["budget"]),
            "test": (lambda task: lambda d: _test_result(run_hidden_test(d, task)))(t),
            "budget": t["budget"],
            "lang": t["lang"],
            "repo": t["repo"],
            "gen9_task": t,
        })
    return tasks


def _test_result(res):
    passed, detail = res
    return f"rc=0 PASS\n{detail[-300:]}" if passed else f"STATUS: FAIL\n{detail[-1400:]}"
