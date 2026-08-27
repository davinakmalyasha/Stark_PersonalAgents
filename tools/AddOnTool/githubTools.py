"""AddOnTool GitHub CLI/API and repository-gate tools.

tools.py state and cross-domain calls are imported lazily inside functions to
avoid an import cycle while preserving the legacy facade namespace.
"""
import json
import os
import re
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
    "python": ["sast", "sca", "dependency-review", "secrets", "quality",
               "coverage", "perf-smoke", "semgrep", "container", "codeql", "zizmor"],
    "node": ["sca", "dependency-review", "secrets", "quality", "coverage", "bundle",
             "perf-smoke", "semgrep", "container", "codeql", "zizmor"],
    "go": ["sca", "dependency-review", "secrets", "tests", "codeql", "zizmor"],
    "php": ["sca", "dependency-review", "secrets", "tests", "codeql", "zizmor"],
    "generic": ["sca", "dependency-review", "secrets", "tests", "codeql", "zizmor"],
}

# Tiered gate mapped to branch levels: T1 fast-checks on dev/feature pushes,
# T2 staging on the staging-level push, T3 release-heavy on prod pushes + tags.
# The shim composes T1+T3; staging.yml is seeded alongside so the full ladder
# exists in every gated repo. Branch names come from .harness-ci.json.
# Branch-level names resolve via the shared <root>/branch_levels.py policy
# module — candidates and fallbacks live there, not here.
_LEVELS_CONFIG = ".harness-ci.json"

_GATE_PROFILE_TEMPLATES = {
    "python": {"security": "github-ci-security-python", "ci": None,
               "staging": "github-staging",
               "gitignore": "gitignore-python", "perf": None, "requirements": True},
    "node": {"security": "github-ci-security-node", "ci": None,
             "staging": "github-staging",
             "gitignore": "gitignore-node", "perf": None, "requirements": False},
    "go": {"security": "github-ci-security-generic", "ci": None,
           "staging": "github-staging",
           "gitignore": None, "perf": None, "requirements": False},
    "php": {"security": "github-ci-security-generic", "ci": None,
            "staging": "github-staging",
            "gitignore": None, "perf": None, "requirements": False},
    "generic": {"security": "github-ci-security-generic", "ci": None,
                "staging": "github-staging",
                "gitignore": None, "perf": None, "requirements": False},
}

def _repo_default_branch(repo: str) -> str:
    out = _gh_api([f"repos/{repo}", "--jq", ".default_branch"])
    if out.startswith("Error") or not out.strip():
        return "main"
    return out.strip()

def _resolve_branch_levels(repo: str) -> dict:
    """Resolve the repo's 3-level branch map via the SHARED policy module
    (branch_levels.py) so the installer and the harness auto-push agree.

    Precedence: .harness-ci.json (via API) > env > remote branch scan > defaults.
    Prod always includes main+master+detected default.
    """
    try:
        import branch_levels as bl
    except ImportError:
        bl = None
    declared = {}
    out = _gh_api([f"repos/{repo}/contents/{_LEVELS_CONFIG}", "--jq", ".content"])
    if not out.startswith("Error") and out.strip() and out.strip() != "null":
        try:
            from base64 import b64decode
            data = json.loads(b64decode(out.replace("\n", "")).decode("utf-8"))
            br = data.get("branches") or {}
            raw = br.get("prod")
            declared = {
                "dev": str(br.get("dev") or "").strip(),
                "staging": str(br.get("staging") or "").strip(),
                "prod": [str(x).strip() for x in
                         (raw if isinstance(raw, list) else [raw]) if str(x).strip()],
                "auto_push": bool((data.get("harness") or {}).get("auto_push", True)),
            }
        except Exception:
            declared = {}
    heads_out = _gh_api([f"repos/{repo}/branches?per_page=100",
                         "--jq", r"[.[].name] | join(\",\")"])
    heads = [] if heads_out.startswith("Error") else \
        [h.strip() for h in heads_out.split(",") if h.strip()]
    default = _repo_default_branch(repo)
    if bl is not None:
        # The installer names things; it does not own pushing, so the kill
        # switch must not blank out resolution here (respect_kill=False).
        lv = bl.normalize(declared, bl.env_overrides(), heads=heads,
                          default=default, respect_kill=False)
        return {"dev": lv["dev"], "staging": bl.render_staging_name(lv),
                "prod": lv["prod"], "declared": bool(declared)}
    # stdlib-only fallback mirroring branch_levels.normalize (legacy runtimes)
    prod = {p for p in declared.get("prod", []) if p} or set()
    for p in (default, "main", "master"):
        prod.add(p)
    dev = declared.get("dev") or next((c for c in ("development", "develop", "dev") if c in heads), "")
    staging = declared.get("staging") or next(
        (c for c in ("staging", "stage", "stg", "qa", "uat", "test") if c in heads), "")
    return {"dev": dev or "development",
            "staging": staging or "staging",
            "prod": sorted(prod), "declared": bool(declared)}

def _ensure_branch(repo: str, branch: str, base_sha: str) -> str:
    """Create refs/heads/<branch> at base_sha when missing (idempotent)."""
    chk = _gh_api([f"repos/{repo}/git/ref/heads/{branch}", "--jq", ".ref"])
    if not chk.startswith("Error") and chk.strip():
        return f"{branch}: exists"
    out = _gh_api(["-X", "POST", f"repos/{repo}/git/refs",
                   "-f", f"ref=refs/heads/{branch}", "-f", f"sha={base_sha}"])
    if out.startswith("Error"):
        return f"{branch}: FAILED ({str(out)[:90]})"
    return f"{branch}: created"

def _ensure_branch_ladder(repo: str, levels: dict) -> str:
    """Idempotently create dev-level and staging-level branches from the default."""
    base_ref = _repo_default_branch(repo)
    base = _gh_api([f"repos/{repo}/git/ref/heads/{base_ref}", "--jq", ".object.sha"])
    if not base.strip().startswith("Error") and len(base.strip()) == 40:
        results = [_ensure_branch(repo, b, base.strip())
                   for b in (levels["dev"], levels["staging"])
                   if b and b not in levels["prod"]]
        return "Branch ladder: " + "; ".join(results)
    return f"Branch ladder skipped (base '{base_ref}' unresolved)."

def _set_default_branch(repo: str, branch: str) -> str:
    """Switch the repo's default branch (used for fresh repos only)."""
    out = _gh_api(["-X", "PATCH", f"repos/{repo}", "-f", f"default_branch={branch}"])
    if out.startswith("Error"):
        return f"Default branch stays (PATCH failed): {str(out)[:100]}"
    return f"Default branch switched to {branch}."

def _job_ids_from_yaml(text: str) -> list[str]:
    """Top-level job ids under `jobs:` — these are the check-run context names."""
    ids, in_jobs = [], False
    for ln in text.splitlines():
        if in_jobs:
            m = re.match(r"^  ([A-Za-z0-9_-]+):\s*(?:#.*)?$", ln)
            if m:
                ids.append(m.group(1))
            elif ln.strip() and not ln.startswith("  "):
                break
        elif ln.rstrip() == "jobs:":
            in_jobs = True
    return ids

def _workflow_job_ids(repo: str, path: str, ref: str = "") -> list[str]:
    """Fetch a workflow file from the repo and extract its job ids."""
    from urllib.parse import quote
    url = f"repos/{repo}/contents/{quote(path)}"
    if ref:
        url += f"?ref={ref}"
    out = _gh_api([url, "--jq", ".content"])
    if out.startswith("Error") or not out.strip() or out.strip() == "null":
        return []
    try:
        from base64 import b64decode
        text = b64decode(out.replace("\n", "")).decode("utf-8")
        return _job_ids_from_yaml(text)
    except Exception:
        return []

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
               ref: str = "", levels: dict | None = None,
               force_levels: bool = False) -> str:
    """Seed the gate files into a repo via the Contents API, rendered from the
    template library. NON-DESTRUCTIVE: every file is skip-if-exists — existing
    project files (requirements, README, .gitignore, workflows, tests) are
    kept untouched and reported.

    force_levels=True: the branch-level-owned files (.harness-ci.json and the
    three tier workflow ymls) are OVERWRITTEN with the freshly resolved names
    so a renamed ladder actually propagates. Everything else stays
    skip-if-exists. Branch-level names are injected into the workflow triggers.
    """
    from tools import apply_template
    from base64 import b64encode
    import tempfile
    if not ref:
        ref = _repo_default_branch(repo)
    tpl = _GATE_PROFILE_TEMPLATES.get(profile)
    if tpl is None:
        return f"Error: unknown profile '{profile}'."
    lv = levels or _resolve_branch_levels(repo)
    prod_csv = ",".join(lv["prod"])
    tmp = tempfile.mkdtemp(prefix="gate-")
    repo_name = repo.split("/")[-1]
    try:
        r1 = apply_template(tpl["security"], target_dir=tmp,
                            variables={"codeql_on": str(codeql_on).lower(),
                                       "staging_branch": lv["staging"],
                                       "prod_branches": prod_csv})
        if r1.startswith("Error"):
            return r1
        if tpl.get("ci"):
            apply_template(tpl["ci"], target_dir=tmp, variables={})
        if tpl.get("staging"):
            apply_template(tpl["staging"], target_dir=tmp,
                           variables={"codeql_on": str(codeql_on).lower(),
                                      "staging_branch": lv["staging"],
                                      "prod_branches": prod_csv})
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
    # Persist the branch-level map so the harness auto-push obeys the same names.
    cfg_path = os.path.join(tmp, _LEVELS_CONFIG)
    if not os.path.exists(cfg_path):
        with open(cfg_path, "w", encoding="utf-8") as f:
            json.dump({
                "_comment": ("Branch-level map consumed by the agent harness "
                             "(auto-push) and the gate tooling. Tiers follow the "
                             "levels: T1=dev/feature pushes, T2=staging push, "
                             "T3=prod push + tags v*."),
                "branches": {"dev": lv["dev"], "staging": lv["staging"],
                             "prod": lv["prod"]},
                "harness": {"auto_push": True},
            }, f, indent=2)
            f.write("\n")
    seeded, kept, errors = [], [], []
    protected_rejects = []  # (path, content) — need the branch+PR fallback
    level_owned = {".github/workflows/fast-checks.yml", ".github/workflows/staging.yml",
                   ".github/workflows/release-heavy.yml", _LEVELS_CONFIG}

    def _put_file(p: str, content: bytes, sha_val: str = "",
                  branch: str = "") -> str:
        encoded = b64encode(content).decode("ascii")
        # Body via --input file: -f content=<b64> blows past the Windows
        # ~32k command-line limit for real workflows (WinError 206).
        body = {"message": "gate: add " + p, "content": encoded}
        if sha_val:
            body["sha"] = sha_val  # REQUIRED when overwriting an existing file
        if branch:
            body["branch"] = branch
        body_path = os.path.join(tmp, ".gh-put-body.json")
        with open(body_path, "w", encoding="utf-8") as bf:
            json.dump(body, bf)
        return _gh_api(["-X", "PUT", f"repos/{repo}/contents/{p}",
                        "-H", "Content-Type: application/json",
                        "--input", body_path])

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
            exists = not sha.startswith("Error") and bool(sha.strip())
            if exists and not (force_levels and p in level_owned):
                kept.append(p)  # skip-if-exists: never overwrite project files
                continue
            out = _put_file(p, content, sha_val=sha.strip() if exists else "")
            if out.startswith("Error"):
                if "required status checks" in out.lower():
                    # Branch is protected — direct writes are impossible by
                    # design. Route through an update branch + PR instead.
                    protected_rejects.append((p, content))
                else:
                    errors.append(f"{p}: {str(out)[:100]}")
            else:
                seeded.append(p)
    if protected_rejects:
        pr_note = _update_levels_via_pr(repo, ref, tmp, protected_rejects)
        seeded.extend(p for p, _ in protected_rejects)
        if pr_note:
            msg_ext = pr_note
        else:
            msg_ext = ""
    else:
        msg_ext = ""
    msg = f"Seeded {len(seeded)} gate files into {repo}@{ref} (profile: {profile})."
    if seeded:
        msg += f"\n  added: {', '.join(seeded[:8])}"
    if kept:
        msg += f"\n  kept (exists, untouched): {', '.join(kept[:8])}"
    if msg_ext:
        msg += f"\n{msg_ext}"
    if errors:
        msg += "\nErrors: " + "; ".join(errors[:5])
    return msg

def _update_levels_via_pr(repo: str, base_ref: str, tmp: str,
                          files: list) -> str:
    """Write level-owned files to an update branch and open a PR into base_ref.

    Needed because branch protection (required status checks) rejects direct
    Contents-API writes to the gated production branch — by design. The PR
    runs the gate itself; merging propagates the repaired files.
    """
    update_branch = "gate/update-levels"
    base = _gh_api([f"repos/{repo}/git/ref/heads/{base_ref}",
                    "--jq", ".object.sha"]).strip()
    if base.startswith("Error") or len(base) != 40:
        return ("PR fallback failed: could not resolve base sha of "
                f"{base_ref}: {base[:80]}")
    _ensure_branch(repo, update_branch, base)
    done = []
    for p, content in files:
        fsha = _gh_api([f"repos/{repo}/contents/{p}?ref={update_branch}",
                        "--jq", ".sha"])
        out = _put_file_local(repo, tmp, p, content,
                              sha_val="" if fsha.startswith("Error") else fsha.strip(),
                              branch=update_branch)
        if not out.startswith("Error"):
            done.append(p)
    if not done:
        return "PR fallback failed: no files written to the update branch."
    pr = _run_gh(["pr", "create", "-R", repo,
                  "--base", base_ref, "--head", update_branch,
                  "--title", "gate: update branch-level files (levels re-render)",
                  "--body", ("Automated gate update: re-rendered "
                             ".harness-ci.json + tier workflows with the current "
                             "branch-level names. Merge to propagate.")])
    if pr.startswith("Error") and "already exists" in pr.lower():
        return (f"Level-owned files updated on branch '{update_branch}' — "
                f"an open PR to {base_ref} already exists: "
                f"https://github.com/{repo}/pulls")
    if pr.startswith("Error"):
        return (f"Level-owned files updated on branch '{update_branch}' but "
                f"PR creation failed: {str(pr)[:120]}")
    return (f"Level-owned files routed via PR (branch '{update_branch}' → "
            f"{base_ref}): {', '.join(done[:6])} — merge it to propagate.")

def _put_file_local(repo: str, tmp: str, p: str, content: bytes,
                    sha_val: str = "", branch: str = "") -> str:
    """Contents-API PUT with a file body (Windows-safe); see _seed_gate."""
    from base64 import b64encode as _b64e
    body = {"message": "gate: update " + p,
            "content": _b64e(content).decode("ascii")}
    if sha_val:
        body["sha"] = sha_val
    if branch:
        body["branch"] = branch
    body_path = os.path.join(tmp, ".gh-put-body.json")
    with open(body_path, "w", encoding="utf-8") as bf:
        json.dump(body, bf)
    return _gh_api(["-X", "PUT", f"repos/{repo}/contents/{p}",
                    "-H", "Content-Type: application/json",
                    "--input", body_path])

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

def _protect_branch(repo: str, branch: str, checks: list) -> str:
    """Require status checks on one branch (blocks merges until green).

    Contexts must be REAL check-run names — extracted from the rendered
    fast-checks.yml (Tier 1 runs on every PR, so contexts stay satisfiable).
    """
    if not branch:
        return ""
    import tempfile
    body = {
        "required_status_checks": {"strict": True, "contexts": list(checks)},
        "enforce_admins": True,
        "required_pull_request_reviews": None,
        "restrictions": None,
    }
    tmp = os.path.join(tempfile.gettempdir(), f"protect-{branch}.json")
    try:
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(body, f)
        out = _gh_api(["-X", "PUT", f"repos/{repo}/branches/{branch}/protection",
                       "--input", tmp])
    except Exception as e:
        return f"Error: protection on {branch} failed: {e}"
    if out.startswith("Error"):
        return f"Error: branch protection on {branch}: {str(out)[:200]}"
    shown = ", ".join(checks[:6]) + ("…" if len(checks) > 6 else "")
    return f"Protected {branch}: requires [{shown}] to pass."

def _protect_default_branch(repo: str, profile: str = "python",
                            codeql_on: bool = True) -> str:
    """Legacy entry point — protect the default branch with the profile checks."""
    branch = _repo_default_branch(repo)
    checks = [c for c in _GATE_CHECKS.get(profile, _GATE_CHECKS["python"])
              if c != "codeql" or codeql_on]
    msg = _protect_branch(repo, branch, checks)
    if not msg.startswith("Error"):
        msg += (" Merge requires these to pass.")
    return msg

def github_apply_gate(repo: str, stack: str = "auto", codeql_on: bool = True,
                      create: bool = False, private: bool = False,
                      description: str = "", gate: str = "ask",
                      update_levels: bool = False) -> str:
    """The ONE gate tool: create the repo (optional), install the tiered gate
    mapped to a 3-level branch ladder (dev/staging/prod): seed CI files
    (non-destructive), ensure the ladder branches exist, wait for the first
    run, enforce branch protection on the production branch.

    update_levels=True re-renders the level-owned files (.harness-ci.json +
    fast-checks/staging/release-heavy ymls) with freshly resolved names — use
    after renaming a company's branch levels; everything else stays
    skip-if-exists.
    """
    created_prefix = ""
    is_new = bool(create)
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

    seed_ref = _repo_default_branch(repo)  # captured BEFORE any default-branch switch
    levels = _resolve_branch_levels(repo)
    seeded = _seed_gate(repo, profile=profile, codeql_on=codeql_on,
                        ref=seed_ref, levels=levels, force_levels=update_levels)
    if seeded.startswith("Error"):
        return seeded
    ladder = _ensure_branch_ladder(repo, levels)

    switched = ""
    if is_new and levels["dev"] != seed_ref:
        switched = _set_default_branch(repo, levels["dev"])

    wait = _wait_for_first_run(repo)

    # Protection: ONLY the prod/default branch gets required checks. dev-level
    # and staging-level branches stay OPEN on purpose — the harness auto-pushes
    # repairs directly to the dev level, and GitHub rejects direct pushes to a
    # branch with required status checks.
    #
    # Contexts are the REAL Tier 1 job ids parsed from the workflow we just
    # seeded (fetched from the seed ref, not the possibly-switched default).
    # Never fall back to legacy name lists: wrong context names permanently
    # lock merges behind checks that can never report.
    ctx = _workflow_job_ids(repo, ".github/workflows/fast-checks.yml", ref=seed_ref)
    if len(ctx) < 3:
        prot_lines = [
            f"WARNING: branch protection SKIPPED — parsed only {len(ctx)} job id(s) "
            f"from fast-checks.yml@{seed_ref}: {ctx}. Inspect the seeded workflow "
            "(invalid render?) before enforcing required checks."]
    else:
        # Protect the PRODUCTION line — never the dev level. On a re-run after
        # the default-branch switch, the runtime default IS the dev level, so
        # resolve the target from the resolved prod set (prefer the original
        # seed ref when it is prod-level, else the first prod branch that
        # exists remotely).
        prot_branch = seed_ref if seed_ref in levels["prod"] else None
        if prot_branch is None:
            for b in levels["prod"]:
                chk = _gh_api([f"repos/{repo}/git/ref/heads/{b}", "--jq", ".ref"])
                if not chk.startswith("Error") and chk.strip():
                    prot_branch = b
                    break
        if prot_branch is None:
            prot_branch = levels["prod"][0] if levels["prod"] else seed_ref
        prot_lines = [_protect_branch(repo, prot_branch, ctx)]

    note = (f"\nNOTES: profile={profile}; branch map dev='{levels['dev']}' "
            f"staging='{levels['staging']}' prod={levels['prod']} "
            f"(persisted as {_LEVELS_CONFIG}; harness auto-push lands on "
            f"'{levels['dev']}' → Tier 1 only; dev/staging branches are "
            "intentionally unprotected so the harness can direct-push). Coverage "
            "threshold and perf budgets are universal defaults — tune after the "
            "first green run.")
    parts = [created_prefix + seeded, ladder]
    if switched:
        parts.append(switched)
    parts.append(wait)
    parts.extend(p for p in prot_lines if p)
    parts.append(note)
    return "\n".join(parts)

def github_init_project(name: str, private: bool = False, description: str = "",
                        gate: str = "ask", stack: str = "auto",
                        codeql_on: bool = True) -> str:
    """Legacy wrapper — merged into github_apply_gate(create=True). Kept importable."""
    return github_apply_gate(name, stack=stack, codeql_on=codeql_on,
                             create=True, private=private, description=description,
                             gate=gate)
