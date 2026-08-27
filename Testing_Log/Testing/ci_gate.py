"""ci_gate.py — push-then-watch GitHub gate loop for delivered project code.

The agents never touch git. After a run that actually changed files in a
workspace, the harness commits and pushes ADAPTIVELY to the repo's dev-level
branch (resolved from .harness-ci.json > env > git discovery > defaults) —
never to production/staging-level branches, so exactly ONE gate tier (Tier 1)
fires per harness push. A background watcher polls the Actions run for that
push:

  - success  -> notify the project chat ("gate passed")
  - failure  -> WAIT until the project's Dev child is idle (never interrupt
                mid-task), then steer Dev with the gate report via
                subagent_manager.send_message (queued if busy). When Dev
                reports DONE the harness pushes again and the loop repeats,
                bounded by CI_GATE_ROUNDS.

No GitHub remote / no gh auth / no changes -> no push, no watch, no block.
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import threading
import time

_GATE_MAX_WAIT = int(os.environ.get("CI_GATE_MAX_WAIT", "1500"))  # s per round
_GATE_POLL = int(os.environ.get("CI_GATE_POLL", "15"))
_CI_ROUNDS = int(os.environ.get("CI_GATE_ROUNDS", "3"))
_STEER_DEADLINE = int(os.environ.get("CI_GATE_STEER_DEADLINE", "7200"))

#: Paths never committed by the harness (secrets / local state / deps).
_ADD_EXCLUDES = [
    "node_modules", "venv", "venv312", ".venv", "__pycache__", ".pytest_cache",
    ".ruff_cache", ".mypy_cache", ".deep_agents", ".agent-memory", ".claude",
    ".codex", ".env", "dist", "build",
]


def _run(cmd, cwd=None, timeout=60):
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    try:
        r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True,
                           timeout=timeout, creationflags=flags)
        return r.returncode, (r.stdout or "") + (r.stderr or "")
    except Exception as e:
        return -1, str(e)


def _has_git(ws: str) -> bool:
    return os.path.isdir(os.path.join(ws, ".git"))


def _git_changes(ws: str) -> list[str]:
    rc, out = _run(["git", "-C", ws, "status", "--porcelain"])
    if rc != 0:
        return []
    return [ln for ln in out.splitlines() if ln.strip()]


# ── Branch-level map: dev / staging / prod (adaptive names, fixed tiers) ────
#
# Resolution policy lives in the shared <root>/branch_levels.py so this harness
# and tools/AddOnTool/githubTools.py can never disagree. This side owns the
# git IO (ls-remote, default-branch detection) and passes signals in.

try:
    import branch_levels as _bl
except ImportError:  # pragma: no cover — exotic runtimes without repo root on path
    _bl = None

_HARNESS_CONFIG = ".harness-ci.json"


def _load_repo_config(ws: str) -> dict:
    """Owner-declared branch levels from .harness-ci.json ({} when absent/bad)."""
    if _bl is not None:
        return _bl.load_repo_config(ws)
    return {}


def _remote_heads(ws: str) -> list[str] | None:
    """All remote branch names via ls-remote (None when git/network fails)."""
    rc, out = _run(["git", "-C", ws, "ls-remote", "--heads", "origin"], timeout=45)
    if rc != 0:
        return None
    prefix = "refs/heads/"
    heads = []
    for ln in out.splitlines():
        ref = ln.split("\t", 1)[1].strip() if "\t" in ln else ""
        if ref.startswith(prefix):
            heads.append(ref[len(prefix):])
    return heads


def _default_branch(ws: str, heads: list[str] | None) -> str:
    """Repo's production default branch — detected, never assumed."""
    rc, out = _run(["git", "-C", ws, "symbolic-ref", "refs/remotes/origin/HEAD"])
    if rc == 0 and out.strip():
        return out.strip().rsplit("/", 1)[-1]
    rc, out = _run(["git", "-C", ws, "remote", "show", "origin"], timeout=60)
    if rc == 0:
        m = re.search(r"HEAD branch:\s*(\S+)", out)
        if m and re.fullmatch(r"[A-Za-z0-9._/\-]+", m.group(1)):
            return m.group(1)  # rejects placeholders like "(unknown)"
    if heads:
        for cand in ("main", "master"):
            if cand in heads:
                return cand
        return heads[0]
    return "main"


def resolve_levels(ws: str) -> dict | None:
    """Resolve the 3-level branch map for a workspace repo.

    Returns {"dev", "staging", "prod": [...], "source", "collision"} — or None
    when auto-push is disabled (.harness-ci.json harness.auto_push=false or
    env CI_GATE_BRANCH=off). staging may be "" when unknown.
    """
    if _bl is None:  # pragma: no cover
        return None
    cfg = _load_repo_config(ws)
    envl = _bl.env_overrides()
    heads = _remote_heads(ws)
    default = _default_branch(ws, heads)
    return _bl.normalize(cfg, envl, heads=heads, default=default, respect_kill=True)


def _current_branch(ws: str) -> str:
    """Checked-out branch name ('' when detached HEAD)."""
    rc, out = _run(["git", "-C", ws, "rev-parse", "--abbrev-ref", "HEAD"])
    br = out.strip() if rc == 0 else ""
    return "" if br == "HEAD" else br


def _push_preview(ws: str, max_list: int = 20) -> str:
    """Visible preview for Dev: what harness will / will not push."""
    raw = _git_changes(ws)
    if not raw:
        return "PUSH PREVIEW: no changes — nothing to commit"
    included: list[str] = []
    excluded: list[str] = []
    for ln in raw:
        # porcelain: XY<space>path[ -> orig]  — path is after 3 chars
        path = ln[3:].strip().split(" -> ")[-1].strip().strip('"')
        is_excluded = False
        for ex in _ADD_EXCLUDES:
            if path == ex or path.startswith(ex + "/") or path.startswith(ex + "\\"):
                is_excluded = True
                break
        (excluded if is_excluded else included).append(f"{ln[:2].strip() or '?'} {path}")
    def _fmt(lst: list[str]) -> str:
        if not lst:
            return "(none)"
        shown = lst[:max_list]
        more = f" …and {len(lst) - max_list} more" if len(lst) > max_list else ""
        return ", ".join(s.split(" ", 1)[-1] for s in shown) + more
    return (
        f"PUSH PREVIEW — harness-owned commit (Dev never runs git push):\n"
        f"  Will commit ({len(included)}): {_fmt(included)}\n"
        f"  Skipped — excluded, never pushed ({len(excluded)}): {_fmt(excluded) if excluded else '(none)'}\n"
        f"  Total working-tree changes: {len(raw)}  |  excluded patterns: {', '.join(_ADD_EXCLUDES[:6])}…\n"
        f"  Dev can also run `git status --porcelain` via run_command at any time (git push itself stays blocked)."
    )


def remote_owner_name(ws: str) -> str | None:
    """Parse 'owner/name' from the workspace's origin remote (or None)."""
    rc, out = _run(["git", "-C", ws, "config", "--get", "remote.origin.url"])
    if rc != 0 or not out.strip():
        return None
    url = out.strip().splitlines()[0]
    m = re.search(r"(?:github\.com[:/])([^/]+)/([^/\s]+?)(?:\.git)?$", url)
    if not m:
        return None
    return f"{m.group(1)}/{m.group(2)}"


def push_if_changed(ws: str, message: str, max_msg: int = 60) -> dict:
    """Commit + push real changes in a workspace. Never touches agents' hands.

    Adaptive targeting: pushes land on the current feature/dev-level branch —
    NEVER on production or staging-level branches (hard blocklist). When the
    workspace sits on prod/detached HEAD, it switches to the dev-level branch
    (created remotely on first push via refspec). One rebase-retry on
    divergence; rejections surface the raw server message once and stop.
    Returns {"pushed", "sha", "repo", "branch", "note", "preview"} — preview
    always present so the harness can surface it to Dev.
    """
    preview = _push_preview(ws)

    def res(**kw: object) -> dict:
        d: dict = {"pushed": False, "sha": "", "repo": None, "branch": "",
                   "note": "", "preview": preview}
        d.update(kw)
        return d

    if not _has_git(ws):
        return res(note="not a git repo")
    if not _git_changes(ws):
        return res(note="no changes")

    levels = resolve_levels(ws)
    if levels is None:
        return res(note="auto-push disabled (.harness-ci.json harness.auto_push=false "
                        "or CI_GATE_BRANCH=off)")
    if levels["dev"] in levels["prod"]:
        # e.g. company's production line IS 'develop' — no safe dev level exists.
        return res(note=f"dev-level '{levels['dev']}' collides with the prod set "
                        f"{levels['prod']} — auto-push skipped (declare levels in "
                        f"{_HARNESS_CONFIG})")

    blocked = set(levels["prod"])
    if levels["staging"]:
        blocked.add(levels["staging"])  # promotion is human/PR-driven, never auto-push
    cur = _current_branch(ws)
    if cur and cur not in blocked:
        target = cur  # feature/dev-level branch respected as-is
    else:
        target = levels["dev"]
        rc, out = _run(["git", "-C", ws, "checkout", "-B", target])
        if rc != 0:
            return res(note=f"cannot switch '{cur or 'detached HEAD'}' -> '{target}': {out[:160]}")

    add = ["git", "-C", ws, "add", "-A", "--", "."] + [f":(exclude){x}" for x in _ADD_EXCLUDES]
    rc, out = _run(add)
    if rc != 0:
        return res(note=f"git add failed: {out[:200]}")

    rc, out = _run(["git", "-C", ws, "commit", "-q", "-m", str(message)[:max_msg]])
    if rc != 0:
        rc2, out2 = _run(["git", "-C", ws, "-c", "user.name=Agent",
                          "-c", "user.email=agent@local", "commit", "-q",
                          "-m", str(message)[:max_msg]])
        if rc2 != 0:
            return res(note=f"commit failed: {(out + out2)[:200]}")
    rc, out = _run(["git", "-C", ws, "rev-parse", "HEAD"])
    sha = out.strip() if rc == 0 else ""

    repo = remote_owner_name(ws)
    if not repo:
        return res(sha=sha, note="no git remote — committed locally only")

    # Pre-flight: does the target ref exist remotely? Refspec-create when new.
    rc, _ = _run(["git", "-C", ws, "ls-remote", "--exit-code", "--heads", "origin", target],
                 timeout=60)
    exists = rc == 0
    refsrc = target if exists else f"HEAD:refs/heads/{target}"
    rc, out = _run(["git", "-C", ws, "push", "-u", "origin", refsrc], timeout=120)
    if rc != 0 and ("non-fast-forward" in out.lower() or "fetch first" in out.lower()):
        # Divergence: one rebase retry, then give up cleanly.
        _run(["git", "-C", ws, "fetch", "origin", target], timeout=120)
        rb_rc, rb_out = _run(["git", "-C", ws, "rebase", f"origin/{target}"])
        if rb_rc == 0:
            rc, out = _run(["git", "-C", ws, "push", "-u", "origin", target], timeout=120)
            if rc == 0:
                # The rebased HEAD is what actually landed — refresh sha so the
                # watcher polls the run of the pushed commit, not the stale one.
                _rc, _head = _run(["git", "-C", ws, "rev-parse", "HEAD"])
                if _rc == 0 and _head.strip():
                    sha = _head.strip()
        else:
            return res(sha=sha, repo=repo, branch=target,
                       note=f"diverged from origin/{target} and rebase failed: {rb_out[:160]}")
    if rc != 0:
        server = next((ln for ln in out.strip().splitlines() if ln.strip()), "")
        return res(sha=sha, repo=repo, branch=target,
                   note=f"push rejected by remote: {server[:220]} (protection/ruleset/auth?)")
    return res(pushed=True, sha=sha, repo=repo, branch=target)


# ── GitHub API (gh CLI) ────────────────────────────────────────────────────

def _gh(args, timeout=45):
    return _run(["gh", "api", *args], timeout=timeout)


def _run_id_for_sha(repo: str, sha: str) -> str | None:
    deadline = time.time() + 90
    while time.time() < deadline:
        rc, out = _gh([f"repos/{repo}/actions/runs?head_sha={sha}&per_page=1",
                       "--jq", ".workflow_runs[0].id"])
        if rc == 0 and out.strip() and out.strip() != "null":
            return out.strip()
        time.sleep(5)
    return None


def _run_state(repo: str, run_id: str) -> tuple[str | None, str | None]:
    rc, out = _gh([f"repos/{repo}/actions/runs/{run_id}",
                   "--jq", '[.status, .conclusion] | @tsv'])
    if rc != 0 or not out.strip():
        return None, None
    parts = out.strip().split("\t")
    return (parts[0] if parts else None), (parts[1] if len(parts) > 1 else None)


def _gate_jobs(repo: str, run_id: str) -> str:
    rc, out = _gh([f"repos/{repo}/actions/runs/{run_id}/jobs", "--jq",
                   '[.jobs[] | .name + ": " + (.conclusion // .status)] | join("\\n")'])
    if rc != 0:
        return ""
    return out.strip()


def _gate_failed_details(repo: str, run_id: str, max_hint_files: int = 12, max_log_tail: int = 80) -> tuple[str, str]:
    """Return (failed_steps_summary, file_hints) for failed jobs — cheap, bounded."""
    # 1) failed jobs + their failed steps (no log download yet)
    rc, out = _gh([f"repos/{repo}/actions/runs/{run_id}/jobs", "--jq",
                   '[.jobs[] | select(.conclusion=="failure") | "\(.name): " + ([.steps[] | select(.conclusion=="failure") | .name] | join(", ")) ] | join("\n")'])
    failed_steps = out.strip() if rc == 0 and out.strip() else ""
    # 2) try to get a short tail of the first failed job's log for file:line hints
    hints = ""
    tail = ""
    try:
        rc2, out2 = _gh([f"repos/{repo}/actions/runs/{run_id}/jobs", "--jq",
                         '[.jobs[] | select(.conclusion=="failure") | .id][0]'])
        job_id = out2.strip() if rc2 == 0 else ""
        if job_id and job_id != "null":
            rc3, log = _gh([f"repos/{repo}/actions/jobs/{job_id}/logs"], timeout=60)
            if rc3 == 0 and log:
                lines = log.splitlines()
                # last N lines, bounded
                tail_lines = lines[-max_log_tail:]
                tail = "\n".join(tail_lines)[-4000:]  # cap 4k
                # extract file:line hints (py/js/ts/go/php) — keep top 12 unique
                seen = []
                seen_set = set()
                pat = re.compile(r'([A-Za-z0-9_\-./\\]+\.(?:py|js|ts|tsx|go|php))(?::|\()(\d+)(?::\d+)?')
                for ln in tail_lines:
                    for m in pat.finditer(ln):
                        key = f"{m.group(1)}:{m.group(2)}"
                        if key not in seen_set:
                            seen_set.add(key)
                            seen.append(key)
                            if len(seen) >= max_hint_files:
                                break
                    if len(seen) >= max_hint_files:
                        break
                if seen:
                    hints = "\n".join(seen)
    except Exception:
        pass
    return failed_steps, hints


# ── Steering Dev (non-interrupting) ────────────────────────────────────────

def _dev_child(ws: str) -> dict | None:
    try:
        from agents.subagent_manager import list_agents
        for a in list_agents(ws):
            if (a.get("name") or "") == "Dev":
                return a
    except Exception as e:
        print(f"[CI_GATE] dev-child lookup error: {e}")
    return None


def _child_busy(child_id: str) -> bool:
    try:
        from agents.subagent_manager import _ACTIVATIONS
        act = _ACTIVATIONS.get(child_id)
        if act is None:
            return False
        with act["lock"]:
            return act["status"] == "running"
    except Exception:
        return False


def _queued_pending(child_id: str) -> int:
    try:
        from agents.subagent_manager import _ACTIVATIONS
        act = _ACTIVATIONS.get(child_id)
        if act is None:
            return 0
        with act["lock"]:
            return len(act["queue"])
    except Exception:
        return 0


def _steer_dev(ws: str, child: dict, repo: str, run_id: str, sha: str,
               branch: str = "") -> str:
    """Wait until Dev is idle, then send the gate report as a steering turn.

    If the child is mid-task the message is queued and runs after the current
    turn — Dev is never interrupted. Enriched payload (failed steps + file:line
    hints + log tail) is bounded to ~4k chars for efficiency.
    """
    jobs = _gate_jobs(repo, run_id) or "(see run page)"
    failed_steps, file_hints = _gate_failed_details(repo, run_id)
    # fetch a short log tail for the first failed job (already capped in helper, re-fetch tail for message)
    log_tail = ""
    if failed_steps:
        # reuse helper's tail via second call to avoid duplicating logic — cheap re-fetch
        try:
            rc, out = _gh([f"repos/{repo}/actions/runs/{run_id}/jobs", "--jq",
                           '[.jobs[] | select(.conclusion=="failure") | .id][0]'])
            jid = out.strip() if rc == 0 else ""
            if jid and jid != "null":
                rc2, log = _gh([f"repos/{repo}/actions/jobs/{jid}/logs"], timeout=60)
                if rc2 == 0 and log:
                    log_tail = "\n".join(log.splitlines()[-40:])[-2500:]
        except Exception:
            pass
    parts = [
        "[CI GATE] Your last push FAILED the GitHub gate — fix and re-verify.",
        f"Repo: {repo}  Push: {sha[:7]}  Run: https://github.com/{repo}/actions/runs/{run_id}",
        f"Jobs:\n{jobs}",
    ]
    if failed_steps:
        parts.append(f"Failed steps:\n{failed_steps}")
    if file_hints:
        parts.append(f"File hints (top 12):\n{file_hints}\n→ open those files first; annotations also appear in PR → Files changed")
    if log_tail:
        parts.append(f"Log tail (last 40 lines, truncated):\n{log_tail}")
    parts.append(
        "Next actions (efficient, in order):\n"
        "1) update_plan(explanation=\"gate failed: <job/step>\", plan=[{step:\"fix <root cause>\", status:\"in_progress\"}, …]) — makes the gate failure a tracked plan step\n"
        "2) run_command: inspect locally if needed (e.g. `pytest -q`, `ruff check .`, `npm test`)\n"
        "3) fix the ROOT CAUSE (smallest complete change), re-run your feature test\n"
        "4) reply EXACTLY: DONE | FILES: <comma-separated changed paths> | NOTE: <one line>\n"
        + (f"Harness re-pushes to dev-level branch '{branch}' when you report DONE — Tier 1 fires only there. "
           "Staging/prod tiers trigger solely via their own branch pushes/tags, never chained."
           if branch else
           "Harness re-pushes to the repo's dev-level branch when you report DONE — Tier 1 only. "
           "Staging/prod tiers trigger solely via their own branch pushes/tags, never chained.")
    )
    msg = "\n\n".join(parts)
    try:
        from agents.subagent_manager import send_message
        deadline = time.time() + _STEER_DEADLINE
        while time.time() < deadline:
            if _child_busy(child["child_id"]):
                time.sleep(10)
                continue
            r = send_message(child["child_id"], msg, ws, timeout=1800)
            if r.get("queued"):
                while time.time() < deadline and _queued_pending(child["child_id"]) > 0:
                    time.sleep(10)
                return "steered (ran after current Dev turn)"
            return "steered: " + str(r.get("result", ""))[:200]
        return "steer timed out (Dev stayed busy)"
    except Exception as e:
        return f"steer error: {e}"


def _notify(ws: str, chat_id: str, text: str) -> None:
    try:
        from workspace_manager import add_message
        add_message(ws, chat_id, "agent", text)
    except Exception as e:
        print(f"[CI_GATE] chat notify error: {e}")


def _log(msg: str) -> None:
    print(f"[CI_GATE] {msg}")
    try:
        from agents.project_router import safe_append_live_log
        safe_append_live_log(f"\n[CI] {msg}")
    except Exception:
        pass


# ── Watch loop ─────────────────────────────────────────────────────────────

def _wait_for_conclusion(repo: str, run_id: str) -> str | None:
    """Poll until the run reaches a terminal conclusion; None on timeout."""
    deadline = time.time() + _GATE_MAX_WAIT
    while time.time() < deadline:
        status, conclusion = _run_state(repo, run_id)
        if status in ("completed",) or conclusion not in (None, "null"):
            return conclusion
        time.sleep(_GATE_POLL)
    return None


def _watch(ws: str, chat_id: str, child: dict | None,
           repo: str, sha: str, run_id: str, branch: str = "") -> None:
    for rnd in range(_CI_ROUNDS):
        conclusion = _wait_for_conclusion(repo, run_id)
        if conclusion is None:
            _notify(ws, chat_id, f"⏳ CI watch timed out on {sha[:7]} — gate still running?")
            return
        if conclusion == "success":
            where = f" on {branch}" if branch else ""
            _notify(ws, chat_id, f"✅ GitHub Tier 1 gate passed — push {sha[:7]}{where} on {repo}.")
            _log(f"gate PASSED for {sha[:7]} ({branch or '?'}) on {repo}")
            return
        if conclusion not in ("failure", "cancelled", "timed_out", "action_required"):
            _notify(ws, chat_id, f"⚠️ Gate state '{conclusion}' on {sha[:7]} — not retrying.")
            return

        _log(f"gate FAILED for {sha[:7]} on {repo} — steering Dev (round {rnd + 1}/{_CI_ROUNDS})")
        _notify(ws, chat_id, f"⛔ GitHub gate failed on {sha[:7]} ({repo}). "
                             "Waiting for Dev to be free, then steering the report to it.")
        if child:
            result = _steer_dev(ws, child, repo, run_id, sha, branch)
            _log(f"steer result: {result[:120]}")
        else:
            _notify(ws, chat_id, "No Dev agent found for this project — gate report needs a manual look.")
            return

        pushed = push_if_changed(ws, f"agent: gate repair round {rnd + 1}")
        preview_r = pushed.get("preview", "")
        if preview_r:
            _log(preview_r)
        if not pushed.get("pushed"):
            _notify(ws, chat_id, f"{preview_r}\n\n{pushed.get('note') or 'No new changes after repair — stopping.'}")
            return
        branch = pushed.get("branch") or branch
        _notify(ws, chat_id, f"{preview_r}\n\nPushed repair {pushed['sha'][:7]} → {branch} — watching gate…")
        sha = pushed["sha"]
        run_id = _run_id_for_sha(repo, sha)
        if run_id is None:
            _notify(ws, chat_id, f"Pushed repair {sha[:7]} but could not find its run — checking GitHub.")
            return
    _notify(ws, chat_id, f"⚠️ Gate still failing after {_CI_ROUNDS} rounds — ask me to continue.")


def after_run(workspace_path: str, prompt: str = "", chat_id: str = "") -> None:
    """Entry point: after a run finishes, push delivered code + watch the gate.

    Fire-and-forget: never blocks the response. Skips silently when nothing
    changed, there is no git repo, or there is no remote.
    """
    if not workspace_path or not os.path.isdir(workspace_path) or not _has_git(workspace_path):
        return
    pushed = push_if_changed(workspace_path, f"agent: {(prompt or '').strip()[:60]}")
    preview = pushed.get("preview", "")
    # Always surface the preview — Dev's view of "what would / did push" without touching git.
    if preview:
        _log(preview)
    if not pushed.get("pushed"):
        if pushed.get("note") and pushed["note"] != "no changes":
            _log(f"skip push: {pushed['note']}")
            # still notify the chat so Dev sees the preview + reason
            try:
                proj_preview = preview or "no preview"
                _notify(workspace_path, chat_id, f"{proj_preview}\n\nSkipped push: {pushed['note']}")
            except Exception:
                pass
        return
    sha, repo = pushed["sha"], pushed.get("repo")
    branch = pushed.get("branch") or ""
    # Visible push report for the project chat + Dev's inbox (harness-owned push, Dev never runs git push)
    _notify(workspace_path, chat_id,
            f"{preview}\n\nPushed {sha[:7]} → {branch or 'local'} on {repo} (Tier 1 gate) — watching…")
    if not repo:
        _log(f"committed {sha[:7]} locally — no remote, no gate watch")
        return

    project_chat = chat_id
    child = None
    try:
        from agents.project_router import find_project_by_workspace
        project = find_project_by_workspace(workspace_path)
        if project:
            project_chat = project.get("chat_id") or chat_id
            if project.get("child_id"):
                child = {"child_id": project["child_id"]}
    except Exception:
        pass
    if child is None:
        child = _dev_child(workspace_path)

    run_id = _run_id_for_sha(repo, sha)
    _log(f"pushed {sha[:7]} → {branch} on {repo} (run {run_id}) — watching the Tier 1 gate")
    threading.Thread(target=_watch,
                     args=(workspace_path, project_chat, child, repo, sha, run_id, branch),
                     daemon=True).start()
