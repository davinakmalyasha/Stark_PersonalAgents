"""Shared file-walk, glob-filter, and safe-open helpers for search tools."""
import glob as globmod
import io
import os

IGNORED_DIRS = frozenset({
    "node_modules", "venv", ".venv", "venv312", ".git", "__pycache__",
    ".claude", ".antigravity", ".deep_agents", ".agent-memory", "scratch",
    "vendor", "dist", "build", ".next", ".vscode", ".idea", ".pytest_cache",
    ".mypy_cache",
})

LINE_SCAN_CHARS = 2000
LINE_SHOW_CHARS = 512


def parse_globs(glob_str) -> tuple[list, list]:
    """Split 'a,b,!c' into (['a','b'], ['c'])."""
    include, exclude = [], []
    for part in str(glob_str or "").split(","):
        part = part.strip()
        if not part:
            continue
        if part.startswith("!"):
            part = part[1:].strip()
            if part:
                exclude.append(part)
        else:
            include.append(part)
    return include, exclude


def glob_ok(name: str, rel: str, include: list, exclude: list) -> bool:
    norm = rel.replace("\\", "/")
    if include and not any(
        globmod.fnmatch.fnmatch(name, g) or globmod.fnmatch.fnmatch(norm, g)
        for g in include
    ):
        return False
    if exclude and any(
        globmod.fnmatch.fnmatch(name, g) or globmod.fnmatch.fnmatch(norm, g)
        for g in exclude
    ):
        return False
    return True


def display_path(path: str, base: str) -> str:
    try:
        rel = os.path.relpath(path, base)
    except Exception:
        rel = path
    rel = rel.replace("\\", "/")
    if rel.startswith("../"):
        return path.replace("\\", "/")
    return rel


def looks_binary(head: bytes) -> bool:
    if not head:
        return False
    if b"\x00" in head:
        return True
    bad = sum(1 for b in head if b < 9 or 13 < b < 32)
    return bad / len(head) > 0.3


def open_text(path: str):
    """Open a text read handle, or None when the file looks binary/unreadable."""
    try:
        raw = open(path, "rb")
    except OSError:
        return None
    try:
        head = raw.read(2048)
    except OSError:
        raw.close()
        return None
    if looks_binary(head):
        raw.close()
        return None
    raw.seek(0)
    return io.TextIOWrapper(raw, encoding="utf-8", errors="replace")


def read_text(path: str):
    """Whole-file text, or None for binary/unreadable files."""
    handle = open_text(path)
    if handle is None:
        return None
    try:
        return handle.read()
    except Exception:
        return None
    finally:
        try:
            handle.close()
        except Exception:
            pass


def iter_files(root: str, include: list, exclude: list, single_file: str = None):
    if single_file:
        rel = os.path.basename(single_file)
        if glob_ok(rel, rel, include, exclude):
            yield single_file
        return
    for dirpath, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in IGNORED_DIRS)
        for fname in sorted(files):
            fpath = os.path.join(dirpath, fname)
            rel = os.path.relpath(fpath, root)
            if glob_ok(fname, rel, include, exclude):
                yield fpath


def resolve_root(path: str, ctx):
    """(search_root, single_file, base, error) for an explicit or default path."""
    from tools import shared_state, WORKSPACE, enforce_fs_permission, _sanitize_path

    active_ws = shared_state.get("project_path") or WORKSPACE
    if ctx is not None:
        try:
            active_ws = getattr(ctx, "project_path", None) or active_ws
        except Exception:
            pass
    explicit = bool(str(path or "").strip())
    enforce_fs_permission("read", path if explicit else active_ws)
    if explicit:
        try:
            target = _sanitize_path(path, ctx)
        except Exception as e:
            return None, None, active_ws, f"Error: path not allowed: {path} ({e})"
        if not os.path.exists(target):
            return None, None, active_ws, f"Error: path not found: {path}"
    else:
        target = _sanitize_path(active_ws, ctx)
    single_file = target if os.path.isfile(target) else None
    root = os.path.dirname(single_file) if single_file else target
    if not root or not os.path.isdir(root):
        return None, None, active_ws, f"Error: path not found: {path}"
    return root, single_file, active_ws, None
