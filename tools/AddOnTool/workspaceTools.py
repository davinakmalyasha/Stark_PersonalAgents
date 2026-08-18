"""AddOnTool workspace safety, salvage, retry, and memory tools.

Facade state and cross-tool calls are imported lazily to avoid cycles.
"""
import os
import re
import time

def _snapshot_dir() -> str:
    from tools import shared_state, WORKSPACE
    import uuid
    ws = os.path.abspath(shared_state.get("project_path") or WORKSPACE or ".")
    snap_dir = os.path.join(ws, ".deep_agents", "snapshots")
    os.makedirs(snap_dir, exist_ok=True)
    return snap_dir

def _snapshot_file(file_path: str) -> str:
    """Snapshot existing file content before a mutation. Returns snapshot path or ''."""
    if os.environ.get("SNAPSHOT", "1") != "1":
        return ""
    try:
        import uuid
        import glob
        import hashlib
        norm = os.path.normpath(os.path.abspath(file_path))
        if not os.path.isfile(norm):
            return ""
        with open(norm, "r", encoding="utf-8", errors="replace") as f:
            content = f.read()
        digest = hashlib.sha256(norm.lower().encode("utf-8", "replace")).hexdigest()[:12]
        fname = "".join(c if c.isalnum() or c in "-_." else "_" for c in os.path.basename(norm))[:40] or "file"
        snap_path = os.path.join(
            _snapshot_dir(),
            f"{digest}_{fname}_{time.strftime('%Y%m%d_%H%M%S')}_{uuid.uuid4().hex[:4]}.snap",
        )
        with open(snap_path, "w", encoding="utf-8") as f:
            f.write(content)
        # Bound storage: keep the 5 most recent snapshots per file digest
        try:
            olds = sorted(glob.glob(os.path.join(_snapshot_dir(), f"{digest}_*.snap")))
            for old in olds[:-5]:
                os.remove(old)
        except Exception:
            pass
        return snap_path
    except Exception:
        return ""

def revert_last_change(file_path: str = "") -> str:
    """Restore the most recent snapshot of a file (undo the last write/edit)."""
    from tools import _invalidate_file_cache
    try:
        import glob
        import hashlib
        if not file_path:
            return "Error: revert_last_change requires 'file_path'."
        norm = os.path.normpath(os.path.abspath(file_path))
        digest = hashlib.sha256(norm.lower().encode("utf-8", "replace")).hexdigest()[:12]
        snaps = sorted(glob.glob(os.path.join(_snapshot_dir(), f"{digest}_*.snap")))
        if not snaps:
            return f"Error: No snapshots found for {file_path}."
        latest = snaps[-1]
        with open(latest, "r", encoding="utf-8") as f:
            content = f.read()
        os.makedirs(os.path.dirname(norm) or ".", exist_ok=True)
        with open(norm, "w", encoding="utf-8") as f:
            f.write(content)
        os.remove(latest)
        _invalidate_file_cache(norm)
        return f"[OK] Reverted {file_path} to snapshot {os.path.basename(latest)} ({len(content)} chars restored)."
    except Exception as e:
        return f"Error reverting {file_path}: {e}"

def capture_workspace_delta(max_chars: int = 6000) -> str:
    """Best-effort git diff + status of the active workspace for salvage-on-death.

    Called on hard-stop / safety-ceiling so a dead agent still delivers its
    partial work. Returns '' when the workspace is not a git repo.
    """
    from tools import shared_state, WORKSPACE
    try:
        import subprocess
        ws = os.path.abspath(shared_state.get("project_path") or WORKSPACE or ".")
        r = subprocess.run(
            ["git", "-C", ws, "diff", "--stat"], capture_output=True, text=True, timeout=10
        )
        if r.returncode != 0:
            return ""
        stat = r.stdout.strip()
        r2 = subprocess.run(["git", "-C", ws, "diff"], capture_output=True, text=True, timeout=15)
        diff = (r2.stdout or "").strip()
        if not stat and not diff:
            return ""
        if len(diff) > max_chars:
            diff = diff[:max_chars] + f"\n...[diff truncated at {max_chars} chars]..."
        return f"[UNCOMMITTED WORK SALVAGE]\n{stat}\n\n{diff}"
    except Exception:
        return ""

def launch_parallel_attempts(name: str, task_desc: str, n: int = 2, wait_seconds: int = 300) -> str:
    """Run N independent parallel re-attempts of a task; return the best result.

    Uses the existing async pool (20 workers). Picks the attempt whose report is
    most complete and least failure-marked. Gated by PARALLEL_RETRY (default off).
    """
    from tools import start_async_task, check_async_task
    import time as time_mod
    import re as _re
    if os.environ.get("PARALLEL_RETRY", "0") != "1":
        return ""
    task_desc = str(task_desc or "").strip()
    if not task_desc:
        return ""
    ids = []
    for i in range(n):
        attempt_task = (
            f"{task_desc}\n\n[PARALLEL ATTEMPT {i + 1}/{n}] This is a fresh independent attempt. "
            "Prior attempts failed. Work independently and return your result in the "
            "AGENT HANDOFF REPORT format."
        )
        started = start_async_task(name, attempt_task)
        m = _re.search(r"Task started:\s*(\S+)", started)
        if m:
            ids.append(m.group(1))

    if not ids:
        return ""
    deadline = time_mod.time() + wait_seconds
    results = []
    for tid in ids:
        while time_mod.time() < deadline:
            st = check_async_task(tid, wait_seconds=0)
            if st.startswith("STATUS: DONE") or st.startswith("STATUS: FAILED"):
                break
            time_mod.sleep(2)
        else:
            st = f"STATUS: TIMED OUT after {wait_seconds}s"
        results.append((tid, st))

    def _score(item) -> float:
        txt = item[1]
        low = txt.lower()
        if item[1].startswith("STATUS: FAILED") or "timed out" in low:
            return 0.0
        if "failed" in low or "blocked" in low or "error" in low or "stuck" in low:
            return 1.0
        return 2.0 + min(len(txt), 100000) / 100000.0

    results.sort(key=_score, reverse=True)
    best = results[0][1] if results else "STATUS: FAILED (no parallel attempts completed)"
    if best.startswith("STATUS: DONE"):
        best = best[len("STATUS: DONE"):].strip()
    return best

_REDACT_PATTERNS = [
    (re.compile(r"(sk-[A-Za-z0-9]{16,})"), "sk-***REDACTED***"),
    (re.compile(r"(AKIA[0-9A-Z]{16})"), "AKIA***REDACTED***"),
    (re.compile(r"(ghp_[A-Za-z0-9]{20,})"), "ghp_***REDACTED***"),
    (re.compile(r"(github_pat_[A-Za-z0-9_]{20,})"), "github_pat_***REDACTED***"),
    (re.compile(r"(xox[baprs]-[A-Za-z0-9-]{8,})"), "xox***REDACTED***"),
    (re.compile(r"(?i:Bearer\s+[A-Za-z0-9._~+/=-]{20,})"), "Bearer ***REDACTED***"),
]

_GENERIC_SECRET_PAT = re.compile(
    r"(?i:(?:password|passwd|secret|token|api[_-]?key|client[_-]?secret|private[_-]?key)"
    r"\s*[=:]\s*['\"][^'\"]{8,}['\"])"
)

def redact_secrets(text: str) -> str:
    """Mask known credential shapes in a tool result. Best-effort, cheap."""
    if not text or os.environ.get("REDACT_SECRETS", "1") != "1":
        return text
    try:
        for pat, repl in _REDACT_PATTERNS:
            text = pat.sub(repl, text)

        def _mask(m):
            line = m.group(0)
            eq = line.find("=")
            if eq == -1:
                eq = line.find(":")
            if eq == -1:
                return "***REDACTED***"
            return line[:eq] + "***REDACTED***"

        text = _GENERIC_SECRET_PAT.sub(_mask, text)
    except Exception:
        pass
    return text

def save_memory(key: str = "", value: str = "", scope: str = "features") -> str:
    """Save a note to persistent project memory (.agent-memory/).

    scope="features": a `## <key>` entry in features.md — remembered on every
    future session (decisions, facts, API contracts, gotchas).
    scope="lessons": appended to lessons.md — a failure/blocker lesson that
    future tasks should avoid repeating.
    Persists across sessions and is injected into future system prompts.
    """
    from tools import shared_state, WORKSPACE
    try:
        from memory.memory_manager import update_feature as _mem_feature, append_lesson as _mem_lesson
        ws = shared_state.get("project_path") or WORKSPACE
        key = (key or "").strip()
        value = (value or "").strip()
        if not key:
            return "Error: save_memory requires 'key'."
        if not value:
            return "Error: save_memory requires 'value'."
        if scope == "lessons":
            ok = _mem_lesson(ws, "SAVED", key, value[:800])
        else:
            ok = _mem_feature(ws, key, value[:800])
        return f"[OK] Memory '{key}' saved (scope={scope})." if ok else f"[ERR] Could not save memory '{key}'."
    except Exception as e:
        return f"Error saving memory: {e}"
