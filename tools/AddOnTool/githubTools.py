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

def github_create_repo(name: str, private: bool = False, description: str = "") -> str:
    """Create a GitHub repository using the gh CLI."""
    args = ["repo", "create", name, "--" + ("private" if private else "public")]
    if description:
        args += ["-d", description]
    out = _run_gh(args)
    if out.startswith("Error"):
        return out
    owner = _gh_api(["user", "--jq", ".login"])
    full = f"{owner.strip()}/{name}" if not owner.startswith("Error") else name
    return f"Created {full}."


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
