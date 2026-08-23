"""MainTool._shared — helpers shared across several tools (cross-cutting).

Terminal-output cleaning (run_command/search_code) and the tool-logging
pipeline used by ~all tools. Self-contained: stdlib + state_sync only.
"""
import re
from sync.state_sync import safe_append_live_log


def _clean_terminal_output(text: str) -> str:
    """Strip ANSI escape codes, progress bar overwrites, warnings summary, and collapse blank lines.
    Saves tokens per command output (#5)."""
    # Strip ANSI escape sequences (full pattern: CSI + OSC + other escapes)
    text = re.sub(r'\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])', '', text)
    
    # Strip pytest warnings summary block
    text = re.sub(r'=+ warnings summary =+.*?(?==+ \d+ (?:passed|failed|error|passed|failed|skipped|warnings)|\Z)', '', text, flags=re.DOTALL | re.IGNORECASE)
    
    # Collapse carriage-return overwrites (keep only final segment) and filter out deprecation/runtime warnings
    lines = text.split('\n')
    cleaned = []
    for line in lines:
        if '\r' in line:
            parts = line.split('\r')
            line = parts[-1]  # keep only last overwrite
        # Filter out verbose python warnings to save tokens
        if "warning" in line.lower() and ("deprecation" in line.lower() or "userwarning" in line.lower() or "runtime" in line.lower() or "importwarning" in line.lower()):
            continue
        cleaned.append(line.rstrip())
        
    text = '\n'.join(cleaned)
    # Collapse 3+ consecutive blank lines to 2
    text = re.sub(r'\n{3,}', '\n\n', text)
    return text.strip()


def _log_tool(name: str, args: dict, result: str) -> None:
    """Log tool usage to shared state for the frontend."""
    preview = result[:300] + "..." if len(result) > 300 else result
    safe_append_live_log(f"\n[TOOL] [{name}] {_fmt_args(args)}\n{preview}")


def _fmt_args(args: dict) -> str:
    """Format tool arguments for display."""
    parts = []
    for k, v in args.items():
        if k == "content":
            parts.append(f"{k}=<{len(str(v))} chars>")
        elif k == "old_string":
            parts.append(f"{k}=<{len(str(v))} chars>")
        elif k == "new_string":
            parts.append(f"{k}=<{len(str(v))} chars>")
        else:
            parts.append(f"{k}={v!r}")
    return ", ".join(parts)
