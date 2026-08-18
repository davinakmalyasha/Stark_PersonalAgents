"""MainTool._shared — helpers shared across several tools (cross-cutting).

Terminal-output cleaning (run_command/search_code), syntax validation
(write_file/edit_file), and the tool-logging pipeline used by ~all tools.
Self-contained: stdlib + state_sync only.
"""
import os
import re
import subprocess
from typing import Optional
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


def _validate_syntax(file_path: str) -> Optional[str]:
    """Inspects syntax of written/edited files to prevent syntax bugs."""
    if not os.path.isfile(file_path):
        return None
    
    ext = file_path.split(".")[-1].lower()
    try:
        if ext == "py":
            import ast
            with open(file_path, "r", encoding="utf-8", errors="replace") as f:
                ast.parse(f.read())
        elif ext == "json":
            import json
            with open(file_path, "r", encoding="utf-8", errors="replace") as f:
                json.load(f)
        elif ext in ("js", "jsx"):
            res = subprocess.run(
                ["node", "-c", file_path],
                capture_output=True,
                text=True,
                timeout=5
            )
            if res.returncode != 0:
                return f"JavaScript Syntax Error:\n{res.stderr.strip()}"
        elif ext == "php":
            res = subprocess.run(
                ["php", "-l", file_path],
                capture_output=True,
                text=True,
                timeout=5
            )
            if res.returncode != 0:
                return f"PHP Syntax Error:\n{res.stderr.strip() or res.stdout.strip()}"
    except Exception as e:
        if isinstance(e, SyntaxError):
            return f"Python Syntax Error: {e.msg} at line {e.lineno}, col {e.offset}."
        elif isinstance(e, json.JSONDecodeError):
            return f"JSON Parsing Error: {e.msg} at line {e.lineno}, col {e.colno}."
        # Silently ignore other exceptions (e.g. node not installed)
        pass
    return None
