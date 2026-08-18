"""MainTool.searchCode — the search_code tool.

Holds search_code (ripgrep-style regex content search with an AST-candidate
footer). tools.py state helpers (shared_state, WORKSPACE, enforce_fs_permission,
_sanitize_path) are imported lazily inside the function to avoid an import cycle.
"""
import glob as globmod
import os
import re
from ._shared import _log_tool


def search_code(pattern: str, path: str = "", glob: str = "") -> str:
    """Search file contents using Python regex (ripgrep-style patterns)."""
    from tools import shared_state, WORKSPACE, enforce_fs_permission, _sanitize_path
    active_ws = shared_state.get("project_path") or WORKSPACE
    enforce_fs_permission("read", path if path else active_ws)
    search_dir = _sanitize_path(path) if path else _sanitize_path(active_ws)
    if not os.path.isdir(search_dir):
        search_dir = _sanitize_path(active_ws)

    results = []
    try:
        compiled = re.compile(pattern)
        ignored_dirs = {
            "node_modules", "venv", ".venv", "venv312", ".git", "__pycache__",
            ".claude", ".antigravity", ".deep_agents", "scratch", "vendor",
            "dist", "build", ".next", ".vscode", ".idea", ".pytest_cache", ".mypy_cache"
        }
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
        # â”€â”€ AST candidates footer: rank the distinct files hit so the caller can
        #    decide WHICH to open without scan-and-open-everything. â”€â”€
        try:
            files_hit = []
            seen = set()
            for r in results:
                # result format: "C:\path\file.py:42: line text" â€” rsplit from
                # the right so Windows drive letters don't break path extraction
                rel = r.rsplit(":", 2)[0]
                if rel not in seen:
                    seen.add(rel)
                    files_hit.append(rel)
                if len(files_hit) >= 5:
                    break
            footer = []
            for rel in files_hit:
                try:
                    from memory.repo_map_generator import get_file_signatures
                    _, ext = os.path.splitext(rel)
                    sigs = get_file_signatures(rel, ext.lower(), is_hot=True)
                    if sigs:
                        syms = ", ".join(s[0] for s in sigs[:8])
                        footer.append(f"  {rel} â†’ {syms}")
                except Exception:
                    footer.append(f"  {rel}")
            if footer:
                output += "\n---\n[AST candidates â€” ranked files with their symbols]:\n" + "\n".join(footer)
        except Exception:
            pass
    _log_tool("search_code", {"pattern": pattern, "path": path, "glob": glob}, output[:500])
    return output
