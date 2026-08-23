"""AddOnTool workspace safety, salvage, retry, and memory tools.

Facade state and cross-tool calls are imported lazily to avoid cycles.
"""
import os
import re

def capture_workspace_delta(max_chars: int = 6000) -> str:
    """Best-effort git diff + status of the active workspace for salvage-on-death.

    Called only for diagnostics on cancellation/error handoffs so a partial
    change can be reported. Returns '' when the workspace is not a git repo.
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
