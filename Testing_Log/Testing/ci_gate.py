"""ci_gate.py — push-then-watch GitHub gate loop for delivered project code.

The agents never touch git. After a run that actually changed files in a
workspace, the harness commits and pushes to the project's GitHub remote. A
background watcher polls the GitHub Actions run for that push:

  - success  -> notify the project chat ("gate passed")
  - failure  -> WAIT until the project's Dev child is idle (never interrupt
                mid-task), then steer Dev with the gate report via
                subagent_manager.send_message (queued if busy). When Dev
                reports DONE the harness pushes again and the loop repeats,
                bounded by CI_GATE_ROUNDS.

No GitHub remote / no gh auth / no changes -> no push, no watch, no block.
"""
from __future__ import annotations

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

    Returns {"pushed", "sha", "repo", "note"}. Local commit only when there is
    no remote; fully skipped when the working tree is clean.
    """
    if not _has_git(ws):
        return {"pushed": False, "sha": "", "repo": None, "note": "not a git repo"}
    if not _git_changes(ws):
        return {"pushed": False, "sha": "", "repo": None, "note": "no changes"}

    add = ["git", "-C", ws, "add", "-A", "--", "."] + [f":(exclude){x}" for x in _ADD_EXCLUDES]
    rc, out = _run(add)
    if rc != 0:
        return {"pushed": False, "sha": "", "repo": None, "note": f"git add failed: {out[:200]}"}

    rc, out = _run(["git", "-C", ws, "commit", "-q", "-m", str(message)[:max_msg]])
    if rc != 0:
        rc2, out2 = _run(["git", "-C", ws, "-c", "user.name=Agent",
                          "-c", "user.email=agent@local", "commit", "-q",
                          "-m", str(message)[:max_msg]])
        if rc2 != 0:
            return {"pushed": False, "sha": "", "repo": None,
                    "note": f"commit failed: {(out + out2)[:200]}"}
    rc, out = _run(["git", "-C", ws, "rev-parse", "HEAD"])
    sha = out.strip() if rc == 0 else ""

    repo = remote_owner_name(ws)
    if not repo:
        return {"pushed": False, "sha": sha, "repo": None,
                "note": "no git remote — committed locally only"}
    rc, out = _run(["git", "-C", ws, "push", "origin", "HEAD"], timeout=120)
    if rc != 0:
        return {"pushed": False, "sha": sha, "repo": repo,
                "note": f"push failed: {out[:200]} (gh/git auth?)"}
    return {"pushed": True, "sha": sha, "repo": repo, "note": ""}


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


def _steer_dev(ws: str, child: dict, repo: str, run_id: str, sha: str) -> str:
    """Wait until Dev is idle, then send the gate report as a steering turn.

    If the child is mid-task the message is queued and runs after the current
    turn — Dev is never interrupted.
    """
    jobs = _gate_jobs(repo, run_id) or "(see run page)"
    msg = (
        "[CI GATE] Your last push FAILED the GitHub gate.\n"
        f"Repo: {repo}\nPush: {sha[:7]}\n"
        f"Run: https://github.com/{repo}/actions/runs/{run_id}\n"
        f"Jobs:\n{jobs}\n\n"
        "Read the failing jobs with run_command (`gh api repos/"
        f"{repo}/actions/runs/{run_id}/jobs`), find the ROOT CAUSE, fix it, "
        "verify with your own feature tests, and reply with EXACTLY:\n"
        "DONE | FILES: <comma-separated changed paths> | NOTE: <one line>\n"
        "The harness pushes again when you report done."
    )
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
           repo: str, sha: str, run_id: str) -> None:
    for rnd in range(_CI_ROUNDS):
        conclusion = _wait_for_conclusion(repo, run_id)
        if conclusion is None:
            _notify(ws, chat_id, f"⏳ CI watch timed out on {sha[:7]} — gate still running?")
            return
        if conclusion == "success":
            _notify(ws, chat_id, f"✅ GitHub gate passed — push {sha[:7]} on {repo}.")
            _log(f"gate PASSED for {sha[:7]} on {repo}")
            return
        if conclusion not in ("failure", "cancelled", "timed_out", "action_required"):
            _notify(ws, chat_id, f"⚠️ Gate state '{conclusion}' on {sha[:7]} — not retrying.")
            return

        _log(f"gate FAILED for {sha[:7]} on {repo} — steering Dev (round {rnd + 1}/{_CI_ROUNDS})")
        _notify(ws, chat_id, f"⛔ GitHub gate failed on {sha[:7]} ({repo}). "
                             "Waiting for Dev to be free, then steering the report to it.")
        if child:
            result = _steer_dev(ws, child, repo, run_id, sha)
            _log(f"steer result: {result[:120]}")
        else:
            _notify(ws, chat_id, "No Dev agent found for this project — gate report needs a manual look.")
            return

        pushed = push_if_changed(ws, f"agent: gate repair round {rnd + 1}")
        if not pushed.get("pushed"):
            _notify(ws, chat_id, pushed.get("note") or "No new changes after repair — stopping.")
            return
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
    if not pushed.get("pushed"):
        if pushed.get("note") and pushed["note"] != "no changes":
            _log(f"skip push: {pushed['note']}")
        return
    sha, repo = pushed["sha"], pushed.get("repo")
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
    _log(f"pushed {sha[:7]} to {repo} (run {run_id}) — watching the gate")
    threading.Thread(target=_watch,
                     args=(workspace_path, project_chat, child, repo, sha, run_id),
                     daemon=True).start()
