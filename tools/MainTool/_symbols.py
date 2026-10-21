"""Enclosing-symbol lookup for search result annotation and snippets.

Python files are parsed exactly with the stdlib ``ast`` (no dependencies).
Other supported extensions fall back to the repo-map signature extractor
(tree-sitter when installed, regex otherwise), which is best-effort.

Results are cached by (path, mtime_ns, size) with a small LRU; every public
function is failure-safe and returns None/empty rather than raising.
"""
import ast
import os
from collections import OrderedDict

_CACHE_MAX = 256
_CACHE: "OrderedDict[tuple, dict]" = OrderedDict()


def _cache_key(path: str):
    try:
        st = os.stat(path)
        return (path, st.st_mtime_ns, st.st_size)
    except OSError:
        return None


def _cache_get(key):
    value = _CACHE.get(key)
    if value is not None:
        _CACHE.move_to_end(key)
    return value


def _cache_put(key, value):
    _CACHE[key] = value
    _CACHE.move_to_end(key)
    while len(_CACHE) > _CACHE_MAX:
        _CACHE.popitem(last=False)


def symbols_for(path: str):
    """Symbol table for a file: {"symbols": [...], "total_lines": int} or None.

    Each symbol: {"name": qualified, "kind": "def"|"class", "start": int, "end": int}.
    """
    key = _cache_key(path)
    if key is None:
        return None
    cached = _cache_get(key)
    if cached is not None:
        return cached
    ext = os.path.splitext(path)[1].lower()
    try:
        data = _python_symbols(path) if ext == ".py" else _signature_symbols(path, ext)
    except Exception:
        data = None
    if data is not None:
        _cache_put(key, data)
    return data


def enclosing(path: str, line: int):
    """Innermost symbol containing a 1-based line, or None."""
    data = symbols_for(path)
    if not data:
        return None
    best = None
    for sym in data.get("symbols") or []:
        start = sym.get("start") or 0
        end = sym.get("end")
        if start <= line and (end is None or line <= end):
            if best is None or start >= (best.get("start") or 0):
                best = sym
    return best


def annotation_for(path: str, line: int) -> str:
    """Short '[kind name]' prefix for a matched line, or ''."""
    sym = enclosing(path, line)
    if not sym:
        return ""
    name = sym.get("name") or ""
    if not name:
        return ""
    return f"[{sym.get('kind') or 'def'} {name}]"


def span_lines(path: str, start: int, end: int, max_lines: int = 40,
               max_chars: int = 2000):
    """Read a symbol body for snippet mode.

    Returns ([(line_no, text)], truncated_count). Truncated_count is the number
    of source lines dropped by the caps (0 when the whole span fits).
    """
    from ._filewalk import read_text, LINE_SHOW_CHARS

    text = read_text(path)
    if text is None:
        return [], 0
    lines = text.splitlines()
    last = min(int(end or start), len(lines))
    out = []
    used = 0
    truncated = 0
    line_no = int(start)
    while line_no <= last:
        if len(out) >= max_lines or used >= max_chars:
            truncated = last - line_no + 1
            break
        shown = lines[line_no - 1][:LINE_SHOW_CHARS]
        out.append((line_no, shown))
        used += len(shown) + 1
        line_no += 1
    return out, truncated


def _python_symbols(path: str):
    from ._filewalk import read_text

    text = read_text(path)
    if text is None:
        return None
    tree = ast.parse(text, filename=path)
    total = text.count("\n") + 1
    symbols = []

    def visit(node, prefix: str):
        for child in getattr(node, "body", []) or []:
            kind = None
            name = None
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                kind, name = "def", child.name
            elif isinstance(child, ast.ClassDef):
                kind, name = "class", child.name
            if not kind:
                continue
            qualified = f"{prefix}{name}"
            symbols.append({
                "name": qualified,
                "kind": kind,
                "start": child.lineno,
                "end": getattr(child, "end_lineno", child.lineno),
            })
            visit(child, qualified + ".")

    visit(tree, "")
    symbols.sort(key=lambda s: (s["start"], s["end"] or s["start"]))
    return {"symbols": symbols, "total_lines": total}


def _signature_symbols(path: str, ext: str):
    try:
        from memory.repo_map_generator import (
            RepoMapGenerator,
            get_file_signatures_with_lines,
        )
    except Exception:
        return None
    if ext not in RepoMapGenerator.SUPPORTED_EXTS:
        return None
    entries = get_file_signatures_with_lines(path, ext, full_depth=True)
    import re as _re

    total = 0
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            for _ in f:
                total += 1
    except Exception:
        total = 0
    symbols = []
    for sig, _depth, _doc, line_no in entries:
        text = str(sig or "").strip()
        kind = "class" if text.startswith(("class ", "interface ", "type ", "enum ", "struct ")) else "def"
        match = _re.search(r"(?:def|class|interface|function|async\s+function)\s+([A-Za-z_$][\w$]*)", text)
        if not match:
            match = _re.match(r"([A-Za-z_$][\w$]*)", text)
        name = match.group(1) if match else text[:60]
        symbols.append({"name": name, "kind": kind, "start": line_no, "end": None})
    symbols.sort(key=lambda s: s["start"])
    for i, sym in enumerate(symbols):
        nxt = symbols[i + 1]["start"] if i + 1 < len(symbols) else (total or sym["start"])
        sym["end"] = max(sym["start"], nxt - 1)
    return {"symbols": symbols, "total_lines": total}
