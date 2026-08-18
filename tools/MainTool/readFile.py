"""MainTool.readFile — the read_file tool.

Holds read_file plus its paging helpers (_apply_read_first_page,
_log_offload_read) and the read-page constants. tools.py state helpers
(enforce_fs_permission, get_vfs_router, _sanitize_path, _FILE_CACHE) are
imported lazily inside read_file to avoid an import cycle.
"""
import json
import os
import time
from typing import Optional
from ._shared import _log_tool


# Claude Code-style Read paging: a whole-file read returns a bounded FIRST PAGE
# (not head+tail â€” the middle of a file is usually the important code). When the
# file is longer, the header carries a PARTIAL view notice pointing at offset/limit
# paging. Explicit offset/limit reads are always honored as requested.
_READ_PAGE_LINES = 250
_READ_PAGE_CHARS = 4000
_PARTIAL_NOTICE = (
    "[PARTIAL view: returned lines {start}-{end} of {total}. "
    "Use read_file(offset={next_offset}) to continue, or search_code to locate specific lines.]\n"
)



def _apply_read_first_page(lines, total, is_full_read):
    """Bound a whole-file read to a first page. Returns (capped_lines, is_partial)."""
    if not is_full_read:
        return lines, False
    capped = False
    if len(lines) > _READ_PAGE_LINES:
        lines = lines[:_READ_PAGE_LINES]
        capped = True
    size = sum(len(ln) for ln in lines)
    while size > _READ_PAGE_CHARS and len(lines) > 1:
        dropped = lines.pop()
        size -= len(dropped)
        capped = True
    return lines, capped or total > len(lines)


def _log_offload_read(file_path: str) -> None:
    """Instrument read-backs of offloaded large outputs (read-back-rate metric)."""
    try:
        if os.environ.get("DEEP_AGENTS_EVAL_RUN") == "1" and (
            "large_tool_results" in file_path or "large_output_" in file_path
        ):
            log_path = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                    ".deep_agents", "gen5_offload_reads.jsonl")
            with open(log_path, "a", encoding="utf-8") as f:
                f.write(json.dumps({"ts": time.time(), "path": file_path[:120]},
                                   ensure_ascii=False) + "\n")
    except Exception:
        pass


def read_file(file_path: str, offset: int = 0, limit: Optional[int] = None) -> str:
    """Read a file with optional offset and limit. Uses session cache (#9)."""
    from tools import enforce_fs_permission, get_vfs_router, _sanitize_path, _FILE_CACHE
    enforce_fs_permission("read", file_path)
    # â”€â”€ VFS Composite Routing â”€â”€
    vfs_path = file_path.replace("\\", "/")
    try:
        vfs_router = get_vfs_router()
        line_offset = max(0, offset - 1) if offset > 0 else 0
        # True total lines from disk for an accurate header (VFS returns at most
        # `limit` lines; the header must report the real file size).
        _real_total = 0
        if offset == 0 and limit is None:
            try:
                _real_path = _sanitize_path(file_path)
                if os.path.isfile(_real_path):
                    with open(_real_path, "r", encoding="utf-8", errors="replace") as _rf:
                        _real_total = len(_rf.readlines())
            except Exception:
                _real_total = 0
        # Full reads return a bounded FIRST PAGE of the file (Claude Code style),
        # with a PARTIAL view notice when the file is longer. The agent pages with
        # offset/limit; the middle is never silently dropped.
        vfs_limit = limit or _READ_PAGE_LINES
        read_res = vfs_router.read(vfs_path, offset=line_offset, limit=vfs_limit)
        if not read_res.error and read_res.file_data is not None:
            content = read_res.file_data["content"]
            lines = content.splitlines(keepends=True)
            total = _real_total or len(lines)
            lines, partial = _apply_read_first_page(lines, total, offset == 0 and limit is None)
            result = "".join(lines)
            start_line = line_offset + 1
            end_line = line_offset + len(lines)
            header = f"[FILE] {file_path} (lines {start_line}-{end_line} of {total})\n"
            if partial:
                header += _PARTIAL_NOTICE.format(start=start_line, end=end_line,
                                                 total=total, next_offset=end_line + 1)
            output = header + result
            # Session cache (M1): a repeat FULL read of an unchanged file returns a
            # compact stub instead of re-emitting the whole content — the file's text
            # is already in the conversation, and re-sending it adds fresh cache-miss
            # tokens for zero new information. offset/limit reads bypass the cache.
            if offset == 0 and limit is None:
                try:
                    # Cache key from the plain absolute path (VFS already did the
                    # read + permission checks; sanitize is only for I/O paths).
                    real_path = os.path.abspath(file_path)
                    mtime = os.path.getmtime(real_path)
                    cache_key = os.path.normpath(real_path).lower()
                    if cache_key in _FILE_CACHE and _FILE_CACHE[cache_key][0] == mtime:
                        _log_tool("read_file", {"file_path": file_path, "cached": True}, output[:300])
                        return (f"{header.splitlines()[0]}\n[CACHED READ â€” file unchanged since "
                                f"your earlier read; full content already in this conversation. "
                                f"Use read_file(offset=..., limit=...) to re-read a specific section.]")
                    _FILE_CACHE[cache_key] = (mtime, output)
                except Exception:
                    pass
            _log_offload_read(file_path)
            _log_tool("read_file", {"file_path": file_path, "offset": offset, "limit": limit}, output[:300])
            return output
    except Exception:
        pass

    path = _sanitize_path(file_path)
    if not os.path.isfile(path):
        return f"Error: File not found: {file_path}"
    try:
        # Check session cache (#9) â€” skip if offset/limit specified (partial read)
        cache_key = os.path.normpath(path).lower()
        if offset == 0 and limit is None:
            try:
                mtime = os.path.getmtime(path)
                if cache_key in _FILE_CACHE:
                    cached_mtime, cached_content = _FILE_CACHE[cache_key]
                    if cached_mtime == mtime:
                        _log_tool("read_file", {"file_path": file_path, "cached": True}, cached_content[:300])
                        # Stub instead of re-emitting the full content: the file is
                        # unchanged and its full text is already in the conversation —
                        # re-sending it would add a fresh cache-miss delta for nothing.
                        head = cached_content.splitlines()[0] if cached_content else f"[FILE] {file_path}"
                        return (f"{head}\n[CACHED READ â€” file unchanged since your earlier read; "
                                f"full content already in this conversation. Use "
                                f"read_file(offset=..., limit=...) to re-read a specific section.]")
            except Exception:
                pass

        with open(path, "r", encoding="utf-8", errors="replace") as f:
            lines = f.readlines()
        total = len(lines)
        start_idx = max(0, offset - 1) if offset > 0 else 0
        lines = lines[start_idx:]

        # Full reads return a bounded FIRST PAGE of the file (Claude Code style),
        # with a PARTIAL view notice when the file is longer. The agent pages with
        # offset/limit; the middle is never silently dropped.
        if limit:
            lines = lines[:limit]
        is_full_read = offset == 0 and not limit
        lines, partial = _apply_read_first_page(lines, total, is_full_read)

        result = "".join(lines)
        if not result.strip():
            result = "(file is empty)"
        start_line = start_idx + 1
        end_line = start_idx + len(lines)
        header = f"[FILE] {file_path} (lines {start_line}-{end_line} of {total})\n"
        if partial:
            header += _PARTIAL_NOTICE.format(start=start_line, end=end_line,
                                             total=total, next_offset=end_line + 1)
        output = header + result

        # Update cache (only for full reads without offset that returned the whole file)
        if is_full_read and not partial:
            try:
                _FILE_CACHE[cache_key] = (os.path.getmtime(path), output)
            except Exception:
                pass

        _log_offload_read(file_path)
        _log_tool("read_file", {"file_path": file_path, "offset": offset, "limit": limit}, output[:300])
        return output
    except Exception as e:
        return f"Error reading {file_path}: {e}"
