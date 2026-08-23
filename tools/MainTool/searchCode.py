"""MainTool.searchCode — the search_code tool.

Holds search_code (ripgrep-style regex content search with an AST-candidate
footer). tools.py state helpers (shared_state, WORKSPACE, enforce_fs_permission,
_sanitize_path) are imported lazily inside the function to avoid an import cycle.
"""
import glob as globmod
import os
import re
from ._shared import _log_tool


def search_code(pattern: str = "", patterns: list = None, path: str = "", glob: str = "", mode: str = "content") -> str:
    """Search code: 'content' (default) regex-matches file contents; 'files' glob-matches filenames. Supports multi-pattern ranked search via patterns=[...]."""
    from tools import shared_state, WORKSPACE, enforce_fs_permission, _sanitize_path
    # Normalize patterns: patterns list takes precedence, else single pattern
    if patterns is not None and isinstance(patterns, list) and len([p for p in patterns if isinstance(p, str) and p.strip()]) > 0:
        # Multi-pattern ranked search
        clean_patterns = [str(p).strip() for p in patterns if isinstance(p, str) and str(p).strip()]
        return _search_multi(clean_patterns, path, glob, mode)
    if patterns is not None and isinstance(patterns, str) and patterns.strip():
        # Allow patterns as single string (comma-separated or single)
        return search_code(pattern=patterns.strip(), path=path, glob=glob, mode=mode)
    if not pattern or not str(pattern).strip():
        return "Error: search_code requires pattern or patterns"
    pattern = str(pattern).strip()

    active_ws = shared_state.get("project_path") or WORKSPACE
    enforce_fs_permission("read", path if path else active_ws)
    search_dir = _sanitize_path(path) if path else _sanitize_path(active_ws)
    if not os.path.isdir(search_dir):
        search_dir = _sanitize_path(active_ws)

    ignored_dirs = {
        "node_modules", "venv", ".venv", "venv312", ".git", "__pycache__",
        ".claude", ".antigravity", ".deep_agents", "scratch", "vendor",
        "dist", "build", ".next", ".vscode", ".idea", ".pytest_cache", ".mypy_cache"
    }

    if mode == "files":
        output = _search_filenames(pattern, search_dir, ignored_dirs, glob)
        _log_tool("search_code", {"pattern": pattern, "path": path, "glob": glob, "mode": mode}, output[:500])
        return output

    results = []
    try:
        compiled = re.compile(pattern)
        for root, dirs, files in os.walk(search_dir):
            dirs[:] = [d for d in dirs if d not in ignored_dirs]
            for fname in files:
                if glob and not globmod.fnmatch.fnmatch(fname, glob):
                    continue
                fpath = os.path.join(root, fname)
                try:
                    with open(fpath, "r", encoding="utf-8", errors="replace") as f:
                        for i, line in enumerate(f, 1):
                            if compiled.search(line):
                                line_text = line.rstrip()
                                if len(line_text) > 200:
                                    line_text = line_text[:200] + "..."
                                results.append(f"{fpath}:{i}: {line_text}")
                                if len(results) >= 50:
                                    break
                    if len(results) >= 50:
                        break
                except Exception:
                    pass
            if len(results) >= 50:
                break
    except re.error as e:
        return f"Error: Invalid regex pattern: {e}"

    if not results:
        output = f"No matches found for pattern: {pattern}"
    else:
        output = f"Found {len(results)} matches:\n" + "\n".join(results)
        # AST candidates footer: rank the distinct files hit so the caller can
        # decide WHICH to open without scan-and-open-everything.
        files_hit = []
        seen = set()
        for r in results:
            # result format: "C:\path\file.py:42: line text" — rsplit from
            # the right so Windows drive letters don't break path extraction
            rel = r.rsplit(":", 2)[0]
            if rel not in seen:
                seen.add(rel)
                files_hit.append(rel)
            if len(files_hit) >= 5:
                break
        output += _ast_footer(files_hit)
    _log_tool("search_code", {"pattern": pattern, "path": path, "glob": glob}, output[:500])
    return output


def _search_multi(patterns: list, path: str = "", glob: str = "", mode: str = "content") -> str:
    """Ranked multi-pattern search: returns files ranked by how many patterns hit."""
    from tools import shared_state, WORKSPACE, enforce_fs_permission, _sanitize_path
    active_ws = shared_state.get("project_path") or WORKSPACE
    enforce_fs_permission("read", path if path else active_ws)
    search_dir = _sanitize_path(path) if path else _sanitize_path(active_ws)
    if not os.path.isdir(search_dir):
        search_dir = _sanitize_path(active_ws)
    ignored_dirs = {
        "node_modules", "venv", ".venv", "venv312", ".git", "__pycache__",
        ".claude", ".antigravity", ".deep_agents", "scratch", "vendor",
        "dist", "build", ".next", ".vscode", ".idea", ".pytest_cache", ".mypy_cache"
    }
    if mode == "files":
        # For files mode, just run each pattern and combine with ranking
        file_scores = {}
        file_why = {}
        for pat in patterns:
            res = _search_filenames(pat, search_dir, ignored_dirs, glob)
            # Parse found files from output
            for line in res.splitlines():
                if line.startswith("Found") or line.startswith("No files") or line.startswith("---") or line.startswith("  "):
                    continue
                rel = line.strip()
                if not rel or "/" in rel or "\\" in rel or "." in rel:
                    if rel and not rel.startswith("Found") and not rel.startswith("No "):
                        file_scores[rel] = file_scores.get(rel, 0) + 10
                        file_why.setdefault(rel, []).append(pat)
        if not file_scores:
            return f"No files found for patterns: {', '.join(patterns)}"
        ranked = sorted(file_scores.items(), key=lambda x: -x[1])[:20]
        out = f"Found {len(ranked)} files ranked for {len(patterns)} patterns:\n"
        for rel, score in ranked:
            why = ", ".join(file_why[rel][:3])
            out += f"  {rel} [score:{score} why:{why}]\n"
        _log_tool("search_code", {"patterns": patterns, "path": path, "mode": mode}, out[:500])
        return out.strip()

    # Content mode: search each pattern and rank files by hit count + why
    file_hits = {}  # fpath -> {"count": int, "patterns": set, "lines": []}
    for pat in patterns:
        try:
            compiled = re.compile(pat)
        except re.error:
            continue
        for root, dirs, files in os.walk(search_dir):
            dirs[:] = [d for d in dirs if d not in ignored_dirs]
            for fname in files:
                if glob and not globmod.fnmatch.fnmatch(fname, glob):
                    continue
                fpath = os.path.join(root, fname)
                try:
                    with open(fpath, "r", encoding="utf-8", errors="replace") as f:
                        for i, line in enumerate(f, 1):
                            if compiled.search(line):
                                if fpath not in file_hits:
                                    file_hits[fpath] = {"count": 0, "patterns": set(), "lines": []}
                                file_hits[fpath]["count"] += 1
                                file_hits[fpath]["patterns"].add(pat)
                                if len(file_hits[fpath]["lines"]) < 3:
                                    txt = line.rstrip()[:120]
                                    file_hits[fpath]["lines"].append(f"{i}: {txt}")
                except Exception:
                    pass
    if not file_hits:
        return f"No matches found for patterns: {', '.join(patterns)}"
    # Rank by number of patterns matched, then total hits
    ranked = sorted(file_hits.items(), key=lambda kv: (-len(kv[1]["patterns"]), -kv[1]["count"]))[:15]
    out = f"Found {len(file_hits)} files ranked for {len(patterns)} patterns:\n"
    for fpath, info in ranked:
        rel = os.path.relpath(fpath, search_dir)
        why = ", ".join(list(info["patterns"])[:3])
        out += f"\n{rel} [hits:{info['count']} patterns:{len(info['patterns'])} why:{why}]\n"
        for ln in info["lines"][:2]:
            out += f"  {ln}\n"
    _log_tool("search_code", {"patterns": patterns, "path": path, "mode": mode}, out[:500])
    return out.strip()


def _search_filenames(pattern: str, search_dir: str, ignored_dirs: set, glob_filter: str) -> str:
    """Glob filename search: returns sorted file paths relative to search_dir."""
    pattern = pattern.replace("\\", "/")
    has_glob_chars = any(ch in pattern for ch in "*?[")

    found = []
    try:
        if has_glob_chars:
            full_pattern = os.path.join(search_dir, pattern)
            for p in globmod.glob(full_pattern, recursive=True):
                if not os.path.isfile(p):
                    continue
                parts = os.path.relpath(p, search_dir).replace("\\", "/").split("/")
                if any(seg in ignored_dirs for seg in parts[:-1]):
                    continue
                if glob_filter and not globmod.fnmatch.fnmatch(os.path.basename(p), glob_filter):
                    continue
                found.append(os.path.relpath(p, search_dir))
                if len(found) >= 100:
                    break
        else:
            needle = pattern.lower()
            for root, dirs, files in os.walk(search_dir):
                dirs[:] = [d for d in dirs if d not in ignored_dirs]
                for fname in files:
                    if glob_filter and not globmod.fnmatch.fnmatch(fname, glob_filter):
                        continue
                    rel = os.path.relpath(os.path.join(root, fname), search_dir)
                    if needle in fname.lower() or needle in rel.replace("\\", "/").lower():
                        found.append(rel)
                    if len(found) >= 100:
                        break
                if len(found) >= 100:
                    break
        found.sort()
    except Exception:
        pass

    if not found:
        return f"No files found matching: {pattern}"
    output = f"Found {len(found)} files:\n" + "\n".join(found)
    output += _ast_footer([os.path.join(search_dir, p) for p in found[:5]])
    return output


def _ast_footer(paths) -> str:
    """AST candidates footer: rank the given files with their symbols."""
    footer = []
    for rel in paths:
        try:
            from memory.repo_map_generator import get_file_signatures
            _, ext = os.path.splitext(rel)
            sigs = get_file_signatures(rel, ext.lower(), is_hot=True)
            if sigs:
                syms = ", ".join(s[0] for s in sigs[:8])
                footer.append(f"  {rel} -> {syms}")
            else:
                footer.append(f"  {rel}")
        except Exception:
            footer.append(f"  {rel}")
    if not footer:
        return ""
    return "\n---\n[AST candidates - ranked files with their symbols]:\n" + "\n".join(footer)
