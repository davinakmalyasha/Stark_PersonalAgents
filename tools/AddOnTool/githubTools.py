"""AddOnTool GitHub CLI/API and repository-gate tools.

tools.py state and cross-domain calls are imported lazily inside functions to
avoid an import cycle while preserving the legacy facade namespace.
"""
import json
import os
import subprocess
import time
from typing import Optional

def _gh_env() -> dict:
    """Build a subprocess env with GH_TOKEN present (env or .env)."""
    env = dict(os.environ)
    token = env.get("GH_TOKEN") or env.get("GITHUB_TOKEN")
    if not token:
        try:
            from dotenv import load_dotenv
            load_dotenv()
            token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
        except Exception:
            token = None
    if token:
        env["GH_TOKEN"] = token
    return env

def _run_gh(args: list, timeout: int = 30) -> str:
    """Run the gh CLI; returns stdout text or an 'Error: ...' string."""
    import subprocess
    import shutil
    gh = shutil.which("gh")
    if not gh:
        for cand in (
            os.path.join(os.environ.get("ProgramFiles", r"C:\Program Files"), "GitHub CLI", "gh.exe"),
            os.path.join(os.environ.get("LocalAppData", ""), "Microsoft", "WinGet", "Links", "gh.exe"),
        ):
            if cand and os.path.isfile(cand):
                gh = cand
                break
    if not gh:
        return "Error: gh CLI not installed. Run: winget install GitHub.cli"
    try:
        r = subprocess.run([gh] + args, capture_output=True, text=True,
                           timeout=timeout, env=_gh_env())
        out = (r.stdout or "").strip()
        err = (r.stderr or "").strip()
        if r.returncode != 0:
            return f"Error: gh {' '.join(args[:4])} failed: {err or out or 'unknown error'}"
        return out or "OK"
    except subprocess.TimeoutExpired:
        return f"Error: gh command timed out after {timeout}s"

def _gh_api(args: list, timeout: int = 30) -> str:
    """Run `gh api` with args and return compact output."""
    return _run_gh(["api"] + args, timeout=timeout)

def github_create_repo(name: str, private: bool = False, description: str = "",
                       gate: str = "ask") -> str:
    """Create a GitHub repository using the gh CLI. After creation, confirms
    with the user whether the security/perf gate should be installed
    (gate: 'ask' | 'yes' | 'no')."""
    args = ["repo", "create", name, "--" + ("private" if private else "public")]
    if description:
        args += ["-d", description]
    out = _run_gh(args)
    if out.startswith("Error"):
        return out
    owner = _gh_api(["user", "--jq", ".login"])
    full = f"{owner.strip()}/{name}" if not owner.startswith("Error") else name
    want = _ask_gate_choice(full, gate)
    if want is True:
        return f"Created {full}.\n{github_apply_gate(full)}"
    return (f"Created {full} without the gate. "
            f"Call github_apply_gate('{full}') if you change your mind.")

def github_create_issue(title: str, body: str = "", repo: str = "", labels: str = "") -> str:
    """Create a GitHub issue. repo: 'owner/name' or empty for the current project's repo."""
    args = ["issue", "create", "-t", title]
    if body:
        args += ["-b", body]
    if labels:
        args += ["-l", labels]
    if repo:
        args += ["-R", repo]
    return _run_gh(args)

def github_list_issues(state: str = "open", limit: int = 10, repo: str = "") -> str:
    """List GitHub issues (state: open|closed|all)."""
    args = ["issue", "list", "-s", state, "-L", str(limit)]
    if repo:
        args += ["-R", repo]
    out = _run_gh(args)
    if out.startswith("Error"):
        return out
    return out or "No issues found."

def _current_repo() -> str:
    """Resolve 'owner/name' from the git remote of the current project."""
    try:
        import subprocess
        r = subprocess.run(["git", "config", "--get", "remote.origin.url"],
                           capture_output=True, text=True, timeout=10)
        url = (r.stdout or "").strip()
        if "github.com/" in url:
            part = url.split("github.com/", 1)[1].replace(".git", "").strip("/")
            if "/" in part:
                return part
    except Exception:
        pass
    return ""

def github_view_issue(issue_number: int, repo: str = "") -> str:
    """View one issue with its comments (title, state, body, discussion)."""
    repo = repo or _current_repo()
    if "/" not in repo:
        return "Error: repo must be 'owner/name' (or run inside the project repo)."
    out = _gh_api([f"repos/{repo}/issues/{issue_number}",
                   "--jq", r'"#\(.number) \(.title) [\(.state)]\nurl: \(.html_url)\nlabels: \([.labels[].name] | join(", "))\n\n\(.body // "")"'])
    if out.startswith("Error"):
        return out
    comments = _gh_api([f"repos/{repo}/issues/{issue_number}/comments",
                        "--jq", r'[.[] | "-- \(.user.login):\n\(.body // "")"] | join("\n\n")'])
    if not comments.startswith("Error") and comments.strip():
        out += "\n\nCOMMENTS:\n" + comments
    return out

def github_close_issue(issue_number: int, repo: str = "", comment: str = "") -> str:
    """Close an issue, optionally with a closing comment."""
    args = ["issue", "close", str(issue_number)]
    if comment:
        args += ["-c", comment]
    if repo:
        args += ["-R", repo]
    return _run_gh(args)

def github_comment_issue(issue_number: int, body: str, repo: str = "") -> str:
    """Add a comment to an issue."""
    args = ["issue", "comment", str(issue_number), "-b", body]
    if repo:
        args += ["-R", repo]
    return _run_gh(args)

def github_list_pull_requests(state: str = "open", limit: int = 10, repo: str = "") -> str:
    """List pull requests (state: open|closed|merged|all)."""
    args = ["pr", "list", "-s", state, "-L", str(limit)]
    if repo:
        args += ["-R", repo]
    out = _run_gh(args)
    if out.startswith("Error"):
        return out
    return out or "No pull requests found."

def github_view_pull_request(pr_number: int, repo: str = "") -> str:
    """View one PR: description, diff stat, mergeability, plus CI check status."""
    repo = repo or _current_repo()
    if "/" not in repo:
        return "Error: repo must be 'owner/name' (or run inside the project repo)."
    out = _gh_api([f"repos/{repo}/pulls/{pr_number}",
                   "--jq", r'"#\(.number) \(.title) [\(.state)] mergeable=\(.mergeable) state=\(.mergeable_state)\nbase: \(.base.ref) <- head: \(.head.ref) +\(.additions) -\(.deletions)\nurl: \(.html_url)\n\n\(.body // "")"'])
    if out.startswith("Error"):
        return out
    head_sha = _gh_api([f"repos/{repo}/pulls/{pr_number}", "--jq", ".head.sha"])
    if not head_sha.startswith("Error") and head_sha.strip():
        checks = _gh_api([f"repos/{repo}/commits/{head_sha.strip()}/check-runs?per_page=50",
                          "--jq", r'[.check_runs[] | "  \(.name): \(.conclusion // .status)"] | join("\n")'])
        if not checks.startswith("Error") and checks.strip():
            out += "\n--- CI CHECKS ---\n" + checks
    return out

def github_create_pull_request(title: str, body: str = "", base: str = "main", head: str = "", repo: str = "") -> str:
    """Create a pull request (head: branch name; empty = current branch)."""
    args = ["pr", "create", "-t", title, "-B", base]
    if body:
        args += ["-b", body]
    if head:
        args += ["-H", head]
    if repo:
        args += ["-R", repo]
    return _run_gh(args)

def github_merge_pull_request(pr_number: int, repo: str = "", method: str = "squash") -> str:
    """Merge a pull request (method: squash|merge|rebase)."""
    args = ["pr", "merge", str(pr_number), "--" + method]
    if repo:
        args += ["-R", repo]
    return _run_gh(args)

def github_search_code(query: str, repo: str = "", limit: int = 10) -> str:
    """Search code across GitHub repos (optionally scoped to one repo)."""
    from urllib.parse import quote
    q = f"{query} repo:{repo}" if repo else query
    out = _gh_api([f"search/code?q={quote(q)}&per_page={limit}",
                   "--jq", r'[.items[] | "\(.repository.full_name): \(.path)"] | join("\n")'])
    if out.startswith("Error"):
        return out
    if not out.strip():
        return ("No matches. (GitHub's code search index may be temporarily "
                "unavailable — retry later, or use github_get_file_contents / "
                "github_search_repositories / github_list_commits instead.)")
    return "Code matches:\n" + out

def github_search_repositories(query: str, limit: int = 10) -> str:
    """Search GitHub repositories by name/description/topic."""
    from urllib.parse import quote
    out = _gh_api([f"search/repositories?q={quote(query)}&per_page={limit}",
                   "--jq", r'[.items[] | "\(.full_name) (stars: \(.stargazers_count)) — \(.description // "")"] | join("\n")'])
    if out.startswith("Error"):
        return out
    if not out.strip():
        return "No matches."
    return "Repos:\n" + out

def github_get_file_contents(path: str, repo: str, ref: str = "", max_chars: int = 8000) -> str:
    """Fetch a single file's contents from a GitHub repo without cloning."""
    if "/" not in repo:
        return "Error: repo must be 'owner/name'."
    from urllib.parse import quote
    url = f"repos/{repo}/contents/{path}"
    if ref:
        url += f"?ref={quote(ref)}"
    out = _gh_api([url, "--jq", ".content"])
    if out.startswith("Error"):
        return out
    from base64 import b64decode
    try:
        text = b64decode(out.replace("\n", "")).decode("utf-8", "replace")
    except Exception:
        return out
    if len(text) > max_chars:
        text = text[:max_chars] + f"\n[... truncated at {max_chars} chars; re-fetch with a more specific path ...]"
    return text

def github_list_commits(repo: str, limit: int = 10, path: str = "") -> str:
    """List recent commits (optionally limited to one path/file)."""
    if "/" not in repo:
        return "Error: repo must be 'owner/name'."
    url = f"repos/{repo}/commits?per_page={limit}"
    if path:
        from urllib.parse import quote
        url += f"&path={quote(path)}"
    out = _gh_api([url, "--jq", r'.[] | "\(.sha[0:7]) \(.commit.author.date[0:10]) \(.commit.message | split("\n")[0])"'])
    if out.startswith("Error"):
        return out
    return out or "No commits."

def github_create_branch(repo: str, branch: str, base: str = "main") -> str:
    """Create a branch on a remote repo (from a base branch)."""
    if "/" not in repo:
        return "Error: repo must be 'owner/name'."
    sha = _gh_api([f"repos/{repo}/git/ref/heads/{base}", "--jq", ".object.sha"])
    if sha.startswith("Error"):
        return sha
    if not sha or len(sha) != 40:
        return "Error: could not resolve base branch sha."
    out = _gh_api(["-X", "POST", f"repos/{repo}/git/refs",
                   "-f", f"ref=refs/heads/{branch}", "-f", f"sha={sha}"])
    if out.startswith("Error"):
        return out
    return f"Created branch {branch} on {repo} (from {base})."

def github_list_workflow_runs(repo: str, limit: int = 5, branch: str = "") -> str:
    """List recent GitHub Actions workflow runs (CI status per run)."""
    if "/" not in repo:
        return "Error: repo must be 'owner/name'."
    url = f"repos/{repo}/actions/runs?per_page={limit}"
    if branch:
        from urllib.parse import quote
        url += f"&branch={quote(branch)}"
    out = _gh_api([url, "--jq", r'.workflow_runs[] | "\(.id) [\(.status)/\(.conclusion)] \(.event) \(.head_branch): \(.display_title)"'])
    if out.startswith("Error"):
        return out
    return out or "No workflow runs."

def github_view_workflow_run(repo: str, run_id: int) -> str:
    """View one CI run: overall status plus per-job results."""
    if "/" not in repo:
        return "Error: repo must be 'owner/name'."
    out = _gh_api([f"repos/{repo}/actions/runs/{run_id}",
                   "--jq", r'"status: \(.status) conclusion: \(.conclusion) branch: \(.head_branch)"'])
    if out.startswith("Error"):
        return out
    jobs = _gh_api([f"repos/{repo}/actions/runs/{run_id}/jobs",
                    "--jq", r'[.jobs[] | "  \(.name): \(.conclusion // .status)"] | join("\n")'])
    if not jobs.startswith("Error") and jobs.strip():
        out += "\nJobs:\n" + jobs
    return out

def github_dispatch_workflow(repo: str, workflow: str, ref: str = "main") -> str:
    """Trigger a GitHub Actions workflow manually (on-demand CI run)."""
    if "/" not in repo:
        return "Error: repo must be 'owner/name'."
    out = _gh_api(["-X", "POST", f"repos/{repo}/actions/workflows/{workflow}/dispatches",
                   "-f", f"ref={ref}"])
    if out.startswith("Error"):
        return out
    return f"Dispatched workflow {workflow} on {repo} @ {ref}."

_GATE_CHECKS = {
    "python": ["sast", "sca", "secrets", "quality", "coverage", "perf-smoke",
               "semgrep", "container", "codeql", "zizmor"],
    "node": ["sca", "secrets", "quality", "coverage", "bundle", "perf-smoke",
             "semgrep", "container", "codeql", "zizmor"],
    "go": ["sca", "secrets", "tests", "codeql", "zizmor"],
    "php": ["sca", "secrets", "tests", "codeql", "zizmor"],
    "generic": ["sca", "secrets", "tests", "codeql", "zizmor"],
}

_GATE_PROFILE_TEMPLATES = {
    "python": {"security": "github-ci-security-python", "ci": "github-ci-python",
               "gitignore": "gitignore-python", "perf": None, "requirements": True},
    "node": {"security": "github-ci-security-node", "ci": None,
             "gitignore": "gitignore-node", "perf": None, "requirements": False},
    "go": {"security": "github-ci-security-generic", "ci": None,
           "gitignore": None, "perf": None, "requirements": False},
    "php": {"security": "github-ci-security-generic", "ci": None,
            "gitignore": None, "perf": None, "requirements": False},
    "generic": {"security": "github-ci-security-generic", "ci": None,
                "gitignore": None, "perf": None, "requirements": False},
}

def _repo_default_branch(repo: str) -> str:
    out = _gh_api([f"repos/{repo}", "--jq", ".default_branch"])
    if out.startswith("Error") or not out.strip():
        return "main"
    return out.strip()

def _detect_stack(repo: str, ref: str = "") -> str:
    """Detect the project stack from the repo tree. Empty/unknown repos
    default to the python profile (safe: all jobs graceful-skip)."""
    if not ref:
        ref = _repo_default_branch(repo)
    tree = _gh_api([f"repos/{repo}/git/trees/{ref}?recursive=1",
                    "--jq", r'[.tree[] | select(.type=="blob") | .path] | join("\n")'])
    if tree.startswith("Error"):
        return "python"
    paths = tree.splitlines()
    if any(p.endswith("package.json") for p in paths):
        return "node"
    if any(p.endswith("composer.json") for p in paths):
        return "php"
    if any(p.endswith("go.mod") for p in paths):
        return "go"
    if any(p.endswith(("pyproject.toml", "requirements.txt", "setup.py")) for p in paths):
        return "python"
    return "generic" if paths else "python"

def _ask_gate_choice(repo: str, mode: str, timeout: int = 120) -> Optional[bool]:
    """Resolve the gate decision. mode: 'ask' (confirm with the human),
    'yes' (apply), 'no' (skip). Returns None when no answer arrived."""
    from tools import shared_state
    from tools import ask_user
    if mode == "yes":
        return True
    if mode == "no":
        return False
    try:
        from user_questions import ask, UserQuestionError
        chat_id = str(shared_state.get("chat_id") or "")
        answers = ask([
            {
                "id": "gate",
                "question": f"Apply the security/performance gate to {repo}?",
                "detail": ("Adds SAST, SCA, secrets, lint+type, coverage, perf, "
                           "semgrep, container, codeql and workflow audits to the "
                           "repo, and blocks merges until they all pass."),
                "header": "Repo gate",
                "options": [
                    {"label": "Yes, apply the gate", "description": "Recommended — every push is security/perf checked"},
                    {"label": "No, skip the gate", "description": "Repo created without checks"},
                ],
            }
        ], timeout=timeout, chat_id=chat_id)
        sel = (answers[0].selected or []) if answers else []
        if sel:
            return sel[0].startswith("Yes")
        return None
    except Exception:
        return None  # no provider / aborted — treated as no answer

def _seed_gate(repo: str, profile: str = "python", codeql_on: bool = True,
               ref: str = "") -> str:
    """Seed the gate files into a repo via the Contents API, rendered from the
    template library. NON-DESTRUCTIVE: every file is skip-if-exists — existing
    project files (requirements, README, .gitignore, workflows, tests) are
    kept untouched and reported."""
    from tools import apply_template
    from base64 import b64encode
    import tempfile
    if not ref:
        ref = _repo_default_branch(repo)
    tpl = _GATE_PROFILE_TEMPLATES.get(profile)
    if tpl is None:
        return f"Error: unknown profile '{profile}'."
    tmp = tempfile.mkdtemp(prefix="gate-")
    repo_name = repo.split("/")[-1]
    try:
        r1 = apply_template(tpl["security"], target_dir=tmp,
                            variables={"codeql_on": str(codeql_on).lower()})
        if r1.startswith("Error"):
            return r1
        if tpl.get("ci"):
            apply_template(tpl["ci"], target_dir=tmp, variables={})
        if tpl.get("gitignore"):
            apply_template(tpl["gitignore"], target_dir=tmp, variables={})
        if tpl.get("requirements"):
            apply_template("requirements", target_dir=tmp, variables={})
        if tpl.get("perf"):
            apply_template(tpl["perf"], target_dir=tmp,
                           variables={"package_name": repo_name})
        apply_template("readme", target_dir=tmp,
                       variables={"project_name": repo_name})
    except Exception as e:
        return f"Error: gate render failed: {e}"
    seeded, kept, errors = [], [], []
    for root, _dirs, fns in os.walk(tmp):
        for fn in sorted(fns):
            p = os.path.relpath(os.path.join(root, fn), tmp).replace("\\", "/")
            try:
                with open(os.path.join(root, fn), "rb") as f:
                    content = f.read()
            except Exception as e:
                errors.append(f"{p}: {e}")
                continue
            sha = _gh_api([f"repos/{repo}/contents/{p}?ref={ref}", "--jq", ".sha"])
            if not sha.startswith("Error") and sha.strip():
                kept.append(p)  # skip-if-exists: never overwrite project files
                continue
            encoded = b64encode(content).decode("ascii")
            args = ["-X", "PUT", f"repos/{repo}/contents/{p}",
                    "-f", "message=gate: add " + p,
                    "-f", "content=" + encoded]
            out = _gh_api(args)
            if out.startswith("Error"):
                if "required status checks" in out.lower():
                    errors.append(f"{p}: default branch is protected — push via a branch + PR (gh api git/refs + gh pr create through run_command)")
                else:
                    errors.append(f"{p}: {str(out)[:100]}")
            else:
                seeded.append(p)
    msg = f"Seeded {len(seeded)} gate files into {repo}@{ref} (profile: {profile})."
    if seeded:
        msg += f"\n  added: {', '.join(seeded[:8])}"
    if kept:
        msg += f"\n  kept (exists, untouched): {', '.join(kept[:8])}"
    if errors:
        msg += "\nErrors: " + "; ".join(errors[:5])
    return msg

def _wait_for_first_run(repo: str, timeout: int = 300) -> str:
    """Poll until the repo has at least one workflow run (the gate's first run)."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        out = _gh_api([f"repos/{repo}/actions/runs?per_page=1", "--jq", ".total_count"])
        if not out.startswith("Error"):
            try:
                if int(out.strip()) > 0:
                    return "First gate run triggered (reported by GitHub Actions)."
            except ValueError:
                pass
        time.sleep(10)
    return "Timed out waiting for the first gate run — protection set anyway (runs on next push)."

def _protect_default_branch(repo: str, profile: str = "python",
                            codeql_on: bool = True) -> str:
    """Require the gate checks on the default branch (blocks merges until green)."""
    import tempfile
    branch = _repo_default_branch(repo)
    checks = [c for c in _GATE_CHECKS.get(profile, _GATE_CHECKS["python"])
              if c != "codeql" or codeql_on]
    body = {
        "required_status_checks": {"strict": True, "contexts": checks},
        "enforce_admins": True,
        "required_pull_request_reviews": None,
        "restrictions": None,
    }
    tmp = os.path.join(tempfile.gettempdir(), "protect.json")
    try:
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(body, f)
        out = _gh_api(["-X", "PUT", f"repos/{repo}/branches/{branch}/protection",
                       "--input", tmp])
    except Exception as e:
        return f"Error: branch protection failed: {e}"
    if out.startswith("Error"):
        return f"Error: branch protection on {branch}: {str(out)[:200]}"
    return (f"Branch protection enforced on {branch}: merge requires "
            f"{', '.join(checks)} to pass.")

def github_apply_gate(repo: str, stack: str = "auto", codeql_on: bool = True,
                      create: bool = False, private: bool = False,
                      description: str = "", gate: str = "ask") -> str:
    """The ONE gate tool: create the repo (optional) and install the security/perf
    gate: seed CI files (non-destructive), wait for the first run, enforce branch
    protection on the default branch.

    create=True: repo is a plain name — the repo is created first (private /
    description apply), then gated. create=False: repo must be 'owner/name'.
    gate: 'ask' (default — confirms with the user for new AND existing repos),
    'yes' (apply without asking), 'no' (skip).
    stack: 'auto' (detect) | python | node | go | php | generic.
    NO templates are applied here — templates are only used when a task
    actually needs them.
    """
    created_prefix = ""
    if create:
        if not repo or "/" in repo:
            return "Error: with create=True, repo must be a plain name (no owner prefix)."
        args = ["repo", "create", repo, "--" + ("private" if private else "public")]
        if description:
            args += ["-d", description]
        out = _run_gh(args)
        if out.startswith("Error"):
            return out
        owner = _gh_api(["user", "--jq", ".login"])
        repo = f"{owner.strip()}/{repo}" if not owner.startswith("Error") else repo
        created_prefix = f"Created {repo}.\n"
    elif "/" not in repo:
        return "Error: repo must be 'owner/name' (or use create=True to create it)."
    want = _ask_gate_choice(repo, gate)
    if want is not True:
        hint = ""
        if want is None:
            hint = "\nNo gate answer received (timeout) — repo left WITHOUT the gate."
        return (f"{created_prefix}{repo} without the gate.{hint}\n"
                f"Call github_apply_gate('{repo}') later to add it.")
    profile = stack if stack != "auto" else _detect_stack(repo)
    if profile not in _GATE_CHECKS:
        return (f"Error: unknown stack '{stack}'. Valid: auto, python, node, "
                f"go, php, generic")
    seeded = _seed_gate(repo, profile=profile, codeql_on=codeql_on)
    if seeded.startswith("Error"):
        return seeded
    wait = _wait_for_first_run(repo)
    protected = _protect_default_branch(repo, profile, codeql_on=codeql_on)
    note = (f"\nNOTES: profile={profile}. Coverage threshold and perf budgets are "
            "universal defaults — tune .github/workflows/security.yml after the "
            "first green run (R4).")
    return f"{created_prefix}{seeded}\n{wait}\n{protected}{note}"

def github_init_project(name: str, private: bool = False, description: str = "",
                        gate: str = "ask", stack: str = "auto",
                        codeql_on: bool = True) -> str:
    """Legacy wrapper — merged into github_apply_gate(create=True). Kept importable."""
    return github_apply_gate(name, stack=stack, codeql_on=codeql_on,
                             create=True, private=private, description=description,
                             gate=gate)
