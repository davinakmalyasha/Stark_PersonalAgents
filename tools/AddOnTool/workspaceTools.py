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
