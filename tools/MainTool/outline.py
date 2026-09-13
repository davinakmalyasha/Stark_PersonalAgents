"""MainTool.outline — signature outline of one file, no bodies.

The pull counterpart to the projectMap Key-file symbols: the map names the
important files, outline(path) gives every signature with definition line
numbers, and the agent reads only the slice it needs via
read_file(offset=line, limit=N). Roughly 1/10 the tokens of a full read.
"""
import os
from ._shared import _log_tool

_OUTLINE_MAX_CHARS = 16000


def _compact_line(sig: str, limit: int = 200) -> str:
    text = " ".join(part.strip() for part in str(sig or "").splitlines() if part.strip())
    text = text.rstrip(";{").rstrip()
    if len(text) > limit:
        text = text[: limit - 3].rstrip() + "..."
    return text


def outline(ctx: object = None, file_path: str = "") -> str:
    """Return every signature in one file with 1-based definition lines.

    Classes/interfaces indent their members; bodies are omitted. ``ctx`` is
    the optional typed WorkspaceDeps (registry passes it when the signature
    starts with ctx/deps/run_ctx); legacy callers omit it.
    """
    if not file_path or not str(file_path).strip():
        return "Error: outline requires file_path"
    file_path = str(file_path).strip()

    from tools import enforce_fs_permission, _sanitize_path
    enforce_fs_permission("read", file_path)
    path = _sanitize_path(file_path, ctx)
    if not os.path.isfile(path):
        from pydantic_ai.exceptions import ToolFailed
        raise ToolFailed(f"Error: File not found: {file_path}")

    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            total_lines = len(f.readlines())
    except Exception as e:
        from pydantic_ai.exceptions import ToolFailed
        raise ToolFailed(f"Error reading {file_path}: {e}")

    ext = os.path.splitext(path)[1].lower()
    from memory.repo_map_generator import (
        get_file_signatures_with_lines,
        RepoMapGenerator,
    )
    supported = RepoMapGenerator.SUPPORTED_EXTS
    if ext not in supported:
        return (f"[OUTLINE] {file_path} — unsupported extension '{ext}'. "
                f"Supported: {', '.join(sorted(supported))}. Use read_file instead.")

    try:
        entries = get_file_signatures_with_lines(path, ext, full_depth=True)
    except Exception as e:
        return f"Error: outline failed for {file_path}: {e}"
    if not entries:
        return f"[OUTLINE] {file_path} — no symbols found; use read_file."

    lines = [f"[OUTLINE] {file_path} — {len(entries)} symbols ({total_lines} lines)"]
    for sig, depth, _doc, line_no in entries:
        text = _compact_line(sig)
        if not text:
            continue
        lines.append(f"{line_no:>6}: " + "  " * max(0, depth) + text)
        if sum(len(ln) for ln in lines) > _OUTLINE_MAX_CHARS:
            lines.append("[truncated; use search_code or read_file for the rest]")
            break
    output = "\n".join(lines)
    _log_tool("outline", {"file_path": file_path}, output[:300])
    return output
