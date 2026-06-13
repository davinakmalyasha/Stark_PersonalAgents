"""
Antigravity Developer Tools â€” Claude Code-style file/code tools.
The Developer agent uses these to read, write, edit, execute, and search code.
"""
import os
import re
import json
import time
import tempfile
import subprocess
import glob as globmod
import threading
import collections
from typing import Optional, Any
from state_sync import shared_state, safe_append_live_log
from contextvars import ContextVar
from deepagents.middleware.filesystem import FilesystemPermission, _check_fs_permission
from subagent_swarm import load_subagent_history, save_subagent_history, clear_subagent_history
from langchain_core.messages import SystemMessage, HumanMessage, AIMessage, ToolMessage

active_permissions: ContextVar[Optional[list[FilesystemPermission]]] = ContextVar("active_permissions", default=None)

# Harness root — env-overridable, defaults to this repo's directory (portable)
_HARNESS_ROOT = os.environ.get("BSA_HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))


def to_virtual_path(path: str) -> str:
    path_str = path.replace("\\", "/")
    if path_str.startswith(("/workspace/", "/scratch/", "/memories/", "/conversation_history/")):
        return "/" + path_str.lstrip("/")
    if path_str in ("/workspace", "/scratch", "/memories", "/conversation_history"):
        return "/" + path_str.lstrip("/")

    # Resolve physical absolute path
    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws = os.path.normpath(os.path.abspath(active_ws)).replace("\\", "/").lower()
    
    abs_path = os.path.normpath(os.path.abspath(path)).replace("\\", "/").lower()
    
    # Check roots
    history_dir = os.path.join(_HARNESS_ROOT, ".deep_agents", "conversation_history")
    history_dir = os.path.normpath(os.path.abspath(history_dir)).replace("\\", "/").lower()
    
    scratch_dir = os.path.join(_HARNESS_ROOT, ".deep_agents", "scratch")
    scratch_dir = os.path.normpath(os.path.abspath(scratch_dir)).replace("\\", "/").lower()
    
    memories_dir = os.path.join(_HARNESS_ROOT, ".deep_agents", "memories")
    memories_dir = os.path.normpath(os.path.abspath(memories_dir)).replace("\\", "/").lower()
    
    if abs_path.startswith(history_dir):
        rel = abs_path[len(history_dir):].lstrip("/")
        return f"/conversation_history/{rel}" if rel else "/conversation_history"
    elif abs_path.startswith(scratch_dir):
        rel = abs_path[len(scratch_dir):].lstrip("/")
        return f"/scratch/{rel}" if rel else "/scratch"
    elif abs_path.startswith(memories_dir):
        rel = abs_path[len(memories_dir):].lstrip("/")
        return f"/memories/{rel}" if rel else "/memories"
    elif abs_path.startswith(active_ws):
        rel = abs_path[len(active_ws):].lstrip("/")
        return f"/workspace/{rel}" if rel else "/workspace"
        
    return f"/workspace/{path.lstrip('/')}"

def enforce_fs_permission(operation: str, path: str) -> None:
    rules = active_permissions.get()
    if rules is None:
        return
    
    virtual_path = to_virtual_path(path)
    mode = _check_fs_permission(rules, operation, virtual_path)
    if mode == "deny":
        raise PermissionError(f"Permission denied for operation '{operation}' on path '{virtual_path}'")
    elif mode == "interrupt":
        raise PermissionError(f"Permission denied (human-in-the-loop approval required but not supported) for operation '{operation}' on path '{virtual_path}'")


def _get_cache_version() -> int:
    from state_sync import active_store
    store = active_store.get()
    if store is not None:
        try:
            item = store.get(("tool_cache",), "version")
            if item is not None and isinstance(item.value, int):
                return item.value
        except Exception:
            pass
    return 0

def _increment_cache_version() -> None:
    from state_sync import active_store
    store = active_store.get()
    if store is not None:
        try:
            current = _get_cache_version()
            store.put(("tool_cache",), "version", current + 1)
        except Exception:
            pass


WORKSPACE = _HARNESS_ROOT


# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
# Token Optimization Helpers
# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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


# Per-site inline-vs-offload caps (chars). Env-tunable: BSA_CAP_<SITE>.
_TOOL_CAP_DEFAULTS = {
    "run_command": 6000,          # test runs: head+tail preview is enough; install noise offloads
    "run_command_timeout": 5000,  # hung-process junk: offload sooner
    "logs": 6000,                 # streaming server logs: poll + page
    "subagent": 6000,             # subagent internal tool results: enough for the NEXT decision, not a dump
}

def _tool_cap(site: str) -> int:
    default = _TOOL_CAP_DEFAULTS.get(site, 9000)
    env_name = f"BSA_CAP_{site.upper()}"
    return int(os.environ.get(env_name, str(default)))


def _create_budgeted_preview(content_str: str, max_chars: int = 2500,
                             head_ratio: float = 0.25, min_lines: int = 4) -> str:
    """Token-budgeted head+tail preview for offloaded tool output.

    Deterministic per-call cost (no 1000-char-line surprise): at most ~max_chars
    chars of preview, split head 25% / tail 75% (errors, tracebacks and last
    assertions live in the tail; the head only needs command context). Individual
    lines are trimmed to 200 chars. Line-numbered so offset/limit paging stays
    precise. Returns the content unchanged when it fits in the budget.
    """
    if len(content_str) <= max_chars:
        return content_str
    from deepagents.middleware._message_eviction import format_content_with_line_numbers
    lines = content_str.splitlines()
    head_budget = max(1.0, max_chars * head_ratio)
    tail_budget = max(1.0, max_chars - head_budget)
    head, tail = [], []
    used_h = used_t = 0
    for ln in lines:
        if used_h + len(ln) + 1 <= head_budget:
            head.append(ln)
            used_h += len(ln) + 1
        else:
            break
    for ln in reversed(lines):
        if used_t + len(ln) + 1 <= tail_budget:
            tail.append(ln)
            used_t += len(ln) + 1
        else:
            break
    # Min-lines enforcement, guarded against head/tail overlap on short content.
    while len(head) < min_lines and len(head) + len(tail) < len(lines):
        head.append(lines[len(head)])
    while len(tail) < min_lines and len(head) + len(tail) < len(lines):
        tail.append(lines[-1 - len(tail)])
    if len(head) + len(tail) > len(lines):
        tail = tail[:max(0, len(lines) - len(head))]
    head = [ln[:200] for ln in head]
    tail = [ln[:200] for ln in tail]
    omitted = len(lines) - len(head) - len(tail)
    head_sample = format_content_with_line_numbers(head, start_line=1)
    tail_sample = format_content_with_line_numbers(tail, start_line=len(lines) - len(tail) + 1)
    truncation_notice = f"\n... [{omitted} lines truncated] ...\n"
    return head_sample + truncation_notice + tail_sample


def auto_offload_result(content_str: str, tool_name: str, max_chars: int = 15000) -> str:
    """If output is too large, offloads full content to VFS scratch and returns TOO_LARGE_TOOL_MSG.

    `max_chars` is per-call. Large command/test output is mostly noise (progress bars,
    npm warnings, directory dumps) â€” the head (command context) and tail (exit code,
    traceback, last assertion) carry the signal. Offloading early keeps the per-turn
    cache-miss input small without losing the full output (it is on disk, readable).
    """
    if len(content_str) <= max_chars:
        return content_str
    import uuid
    from deepagents.middleware._message_eviction import TOO_LARGE_TOOL_MSG

    file_id = uuid.uuid4().hex[:8]
    clean_name = "".join(c if c.isalnum() or c in "-_" else "_" for c in tool_name)[:50]
    vfs_path = f"/scratch/large_tool_results/{clean_name}_{file_id}.txt"
    try:
        vfs_router = get_vfs_router()
        res = vfs_router.write(vfs_path, content_str)
        if res and res.error:
            raise RuntimeError(res.error)
    except Exception as e:
        # Fallback to writing directly to physical scratch directory
        try:
            path = _sanitize_path(vfs_path)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as f:
                f.write(content_str)
        except Exception as e_inner:
            print(f"Error offloading tool message: {e_inner}")
            return content_str[:max_chars] + f"\n... [TRUNCATED DUE TO ERROR: {e_inner}] ..."

    content_sample = _create_budgeted_preview(content_str)
    try:
        from context_compaction import truncation_note
        content_sample += truncation_note(len(content_str), max_chars)
    except Exception:
        pass
    return TOO_LARGE_TOOL_MSG.format(
        tool_call_id=f"{clean_name}_{file_id}",
        file_path=vfs_path,
        content_sample=content_sample
    )


# Session-level file content cache (#9)
# Keyed by normalized path â†’ (mtime, content_result)
# Invalidated when write_file/edit_file/apply_diff modifies a file
_FILE_CACHE: dict[str, tuple[float, str]] = {}

# run_command output dedup (C): command -> md5 of its last output in this session.
# Re-running the same command with byte-identical output returns a stub instead of
# re-appending the full text as fresh cache-miss input.
_RUN_OUTPUT_CACHE: dict[str, str] = {}

# â”€â”€ Pillar 113: Speculative Tool Response Cache â”€â”€
# Key: hash(tool_name:canonical_args) â†’ (timestamp, result)
# Read-only tools (list_files, search_code, read_file, view_signatures) use this.
# TTL: 300s default. Invalidated on any file write.
import hashlib
_TOOL_RESPONSE_CACHE: dict[str, tuple[float, str]] = {}
_TOOL_MTIME_MAP: dict[str, float] = {}
_TOOL_CACHE_TTL = 300  # seconds

def _make_tool_cache_key(tool_name: str, args: dict) -> str:
    """Create a deterministic cache key from tool name and sorted args."""
    raw = tool_name + ":" + json.dumps(args, sort_keys=True, default=str)
    return hashlib.sha256(raw.encode()).hexdigest()

def _check_tool_cache(tool_name: str, args: dict) -> Optional[str]:
    """Check if a read-only tool call has a fresh cached result. Returns None on miss."""
    if tool_name not in ("read_file", "list_files", "search_code", "view_signatures"):
        return None
    now = time.time()
    key = _make_tool_cache_key(tool_name, args)
    entry = _TOOL_RESPONSE_CACHE.get(key)
    if entry is None:
        return None
    ts, result = entry
    if now - ts > _TOOL_CACHE_TTL:
        del _TOOL_RESPONSE_CACHE[key]
        _TOOL_MTIME_MAP.pop(key, None)
        return None
    # For file-based tools, also check mtime hasn't changed
    file_path = args.get("file_path") or args.get("path") or ""
    if file_path:
        actual_path = _sanitize_path(file_path)
        if os.path.isfile(actual_path):
            try:
                current_mtime = os.path.getmtime(actual_path)
                cached_mtime = _TOOL_MTIME_MAP.get(key)
                if cached_mtime is not None and cached_mtime != current_mtime:
                    del _TOOL_RESPONSE_CACHE[key]
                    _TOOL_MTIME_MAP.pop(key, None)
                    return None
            except Exception:
                pass
    return result

def _set_tool_cache(tool_name: str, args: dict, result: str) -> None:
    """Store a read-only tool result in the response cache."""
    if tool_name not in ("read_file", "list_files", "search_code", "view_signatures"):
        return
    key = _make_tool_cache_key(tool_name, args)
    _TOOL_RESPONSE_CACHE[key] = (time.time(), result)
    # Store mtime for invalidation
    file_path = args.get("file_path") or args.get("path") or ""
    if file_path:
        actual_path = _sanitize_path(file_path)
        if os.path.isfile(actual_path):
            try:
                _TOOL_MTIME_MAP[key] = os.path.getmtime(actual_path)
            except Exception:
                pass

def _invalidate_tool_cache_all() -> None:
    """Clear all tool response cache entries (called on any file write)."""
    _TOOL_RESPONSE_CACHE.clear()
    _TOOL_MTIME_MAP.clear()

def _invalidate_file_cache(path: str) -> None:
    """Invalidate cache entry when a file is modified."""
    key = os.path.normpath(path).lower()
    _FILE_CACHE.pop(key, None)
    # Also invalidate tool response cache since file state changed
    _invalidate_tool_cache_all()


class WorkspaceFallbackBackend:
    def __init__(self, active_ws: str, state_backend: Any):
        from deepagents.backends import FilesystemBackend
        self.active_ws = active_ws
        self.state_backend = state_backend
        self.fs_virtual = FilesystemBackend(root_dir=active_ws, virtual_mode=True)
        self.fs_real = FilesystemBackend(root_dir=active_ws, virtual_mode=False)

    def _get_backend(self, path: str) -> Any:
        if not path:
            return self.fs_virtual
        norm_path = path.replace("\\", "/")
        parts = [p for p in norm_path.split("/") if p]
        if parts and parts[0] in ("memories", "scratch"):
            return self.state_backend
        is_drive_abs = len(path) > 1 and path[1] == ':'
        if is_drive_abs:
            return self.fs_real
        return self.fs_virtual

    def ls(self, path: str) -> Any:
        return self._get_backend(path).ls(path)

    def read(self, file_path: str, offset: int = 0, limit: int = 2000) -> Any:
        return self._get_backend(file_path).read(file_path, offset=offset, limit=limit)

    def write(self, file_path: str, content: str) -> Any:
        return self._get_backend(file_path).write(file_path, content)

    def edit(self, file_path: str, old_string: str, new_string: str, replace_all: bool = False) -> Any:
        return self._get_backend(file_path).edit(file_path, old_string, new_string, replace_all)

    def grep(self, pattern: str, path: str | None = None, glob: str | None = None) -> Any:
        return self._get_backend(path).grep(pattern, path, glob)

    def glob(self, pattern: str, path: str | None = None) -> Any:
        return self._get_backend(path).glob(pattern, path)

    def upload_files(self, files: list[tuple[str, bytes]]) -> Any:
        res = []
        for p, c in files:
            res.extend(self._get_backend(p).upload_files([(p, c)]))
        return res

    def download_files(self, paths: list[str]) -> Any:
        res = []
        for p in paths:
            res.extend(self._get_backend(p).download_files([p]))
        return res


def get_vfs_router() -> Any:
    from deepagents.backends import CompositeBackend, StateBackend, FilesystemBackend, StoreBackend
    from state_sync import active_store

    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws = os.path.normpath(os.path.abspath(active_ws))

    store = active_store.get()
    store_backend = StoreBackend(store=store) if store is not None else StoreBackend()

    # Build routes: virtual paths (/workspace/, /memories/, /scratch/) AND
    # the active workspace as a real-filesystem route so absolute paths
    # like D:\MyProject\TestProjectForAgent\planning.md land on disk,
    # not in the ephemeral StateBackend.
    routes = {
        "/workspace/": FilesystemBackend(root_dir=active_ws, virtual_mode=True),
        "/memories/": store_backend,
        "/scratch/": StateBackend(),
    }
    # Add active workspace as a real-filesystem route (virtual_mode=False)
    # so that absolute paths to the sandbox/project directory bypass
    # the in-memory StateBackend and write directly to disk.
    # Use forward slashes for consistent CompositeBackend route matching.
    if active_ws:
        ws_key = active_ws.replace("\\", "/") + "/"
        routes[ws_key] = FilesystemBackend(root_dir=active_ws, virtual_mode=False)

    state_backend = StateBackend()
    return CompositeBackend(
        default=WorkspaceFallbackBackend(active_ws, state_backend),
        routes=routes,
    )


def _sanitize_path_fallback(path: str) -> str:
    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws = os.path.normpath(os.path.abspath(active_ws))
    path_str = path.replace("\\", "/")
    
    if path_str.startswith("/workspace/"):
        rel_path = path_str[len("/workspace/"):]
        real_path = os.path.join(active_ws, rel_path)
    elif path_str.startswith("/scratch/"):
        rel_path = path_str[len("/scratch/"):]
        scratch_dir = os.path.join(_HARNESS_ROOT, ".deep_agents", "scratch")
        os.makedirs(scratch_dir, exist_ok=True)
        real_path = os.path.join(scratch_dir, rel_path)
    elif path_str.startswith("/memories/"):
        rel_path = path_str[len("/memories/"):]
        memories_dir = os.path.join(_HARNESS_ROOT, ".deep_agents", "memories")
        os.makedirs(memories_dir, exist_ok=True)
        real_path = os.path.join(memories_dir, rel_path)
    elif path_str.startswith("/conversation_history/"):
        rel_path = path_str[len("/conversation_history/"):]
        history_dir = os.path.join(_HARNESS_ROOT, ".deep_agents", "conversation_history")
        os.makedirs(history_dir, exist_ok=True)
        real_path = os.path.join(history_dir, rel_path)
    else:
        if not os.path.isabs(path):
            real_path = os.path.join(active_ws, path)
        else:
            real_path = path
            
    real_path = os.path.normpath(os.path.abspath(real_path))
    return real_path


def _sanitize_path(path: str) -> str:
    """Resolve a relative or absolute path within allowed roots, using VFS routing."""
    from deepagents.backends import FilesystemBackend
    
    vfs_router = get_vfs_router()
    resolved_path = None
    try:
        backend, stripped_key = vfs_router._get_backend_and_key(path)
        if isinstance(backend, FilesystemBackend):
            resolved_path = str(backend._resolve_path(stripped_key))
    except Exception:
        pass
        
    if resolved_path is None:
        resolved_path = _sanitize_path_fallback(path)
        
    real_path = os.path.normpath(os.path.abspath(resolved_path))
    
    # Block access to internal metadata folder (.deep_agents / .antigravity) except when reading scratch/memories/chats/history
    path_parts = real_path.replace("\\", "/").split("/")
    lower_parts = [p.lower() for p in path_parts]
    if ".deep_agents" in lower_parts or ".antigravity" in lower_parts:
        if "scratch" not in path_parts and "memories" not in path_parts and "chats" not in path_parts and "conversation_history" not in path_parts:
            raise ValueError("Access to internal metadata folder is restricted.")
        
    # Security: strictly allow only the active workspace, LangChain system directory, scratch, memories, and history
    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws = os.path.normpath(os.path.abspath(active_ws))
    allowed_roots = [
        active_ws,
        WORKSPACE,
        os.path.join(_HARNESS_ROOT, ".deep_agents", "scratch"),
        os.path.join(_HARNESS_ROOT, ".deep_agents", "memories"),
        os.path.join(_HARNESS_ROOT, ".deep_agents", "conversation_history"),
        os.path.join(_HARNESS_ROOT, ".antigravity", "scratch"),
        os.path.join(_HARNESS_ROOT, ".antigravity", "memories"),
        os.path.join(_HARNESS_ROOT, ".antigravity", "conversation_history"),
    ]
    allowed = any(real_path.lower().startswith(r.lower()) for r in allowed_roots)
    if not allowed:
        raise ValueError(f"Path outside allowed roots: {real_path}")
        
    return real_path


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


def _diff_block(file_path: str, before: str, after: str, max_lines: int = 24) -> str:
    """Compact unified diff between two file contents for rich edit results.
    Capped so a huge file never floods the context."""
    try:
        import difflib
        diff = list(difflib.unified_diff(
            (before or "").splitlines(), (after or "").splitlines(),
            fromfile=file_path, tofile=file_path, lineterm="", n=2))
        if len(diff) <= 1:
            return ""
        if len(diff) > max_lines + 8:
            diff = diff[:max_lines] + [f"... ({len(diff) - max_lines} more diff lines) ..."]
        return "```diff\n" + "\n".join(diff) + "\n```"
    except Exception:
        return ""


def _edit_snippet(file_path: str, before: str, after: str, context: int = 5) -> str:
    """Return the NEW file content around the first changed region (±context
    lines, with line numbers) so the model sees its edit in context without
    re-reading the file (the post-edit re-read loop killer)."""
    try:
        import difflib
        b = (before or "").splitlines()
        a = (after or "").splitlines()
        sm = difflib.SequenceMatcher(None, b, a)
        changed = [(i1, i2, j1, j2) for tag, i1, i2, j1, j2 in sm.get_opcodes() if tag != "equal"]
        if not changed:
            return ""
        _, _, j1, j2 = changed[0]
        lo = max(0, j1 - context)
        hi = min(len(a), j2 + context)
        out = []
        if lo > 0:
            out.append(f"... ({lo} lines above, unchanged) ...")
        out += [f"{i + 1}: {l}" for i, l in enumerate(a[lo:hi], start=lo)]
        if hi < len(a):
            out.append(f"... ({len(a) - hi} lines below, unchanged) ...")
        return "```\n" + "\n".join(out) + "\n```"
    except Exception:
        return ""


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



# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
# Tool Definitions (LangChain-compatible)
# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

TOOL_DEFINITIONS = [
    {
        "name": "read_file",
        "description": "Reads a file from the local filesystem. Use this to inspect code, configs, or any project file.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to read."
                },
                "offset": {
                    "type": "integer",
                    "description": "Optional. Line number to start reading from."
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional. Maximum number of lines to read."
                }
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "write_file",
        "description": "Writes a file to the local filesystem. Overwrites if it exists. Use this to create new files or fully replace existing ones.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to write."
                },
                "content": {
                    "type": "string",
                    "description": "The full content to write to the file."
                }
            },
            "required": ["file_path", "content"]
        }
    },
    {
        "name": "edit_file",
        "description": "Edit an existing file using SEARCH/REPLACE blocks. Supports single or multiple blocks in one call. PREFERRED method for all file edits.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to modify."
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to replace (single edit mode). Must match the file content exactly."
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to replace it with (single edit mode)."
                },
                "diff": {
                    "type": "string",
                    "description": "One or more SEARCH/REPLACE blocks (batch edit mode). Format: <<<<<<< SEARCH\\n[old]\\n=======\\n[new]\\n>>>>>>> REPLACE"
                }
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "run_command",
        "description": "Executes a shell command. Can be run in foreground (blocking, streams logs in real-time) or background (non-blocking, returns immediately). Use process_action parameter to list, view logs, or kill background processes.",
        "parameters": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute. Optional if process_action is 'list', but required as the target process name/key for 'logs' or 'kill'."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds for foreground execution. Default 30000."
                },
                "background": {
                    "type": "boolean",
                    "description": "Optional. If true, run command in background and return a key to manage it. Useful for starting servers."
                },
                "process_action": {
                    "type": "string",
                    "description": "Optional action to manage background processes: 'list' (list all), 'logs' (get logs for process named in command), 'kill' (terminate process named in command)."
                }
            },
            "required": []
        }
    },
    {
        "name": "search_code",
        "description": "Searches file contents using Python regex (ripgrep-style patterns). Use this to find functions, classes, imports, or patterns across the codebase.",
        "parameters": {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for in file contents."
                },
                "path": {
                    "type": "string",
                    "description": "Optional directory or file to search in. Defaults to the project root."
                },
                "glob": {
                    "type": "string",
                    "description": "Optional glob pattern to filter files (e.g. '*.py', '*.tsx')."
                }
            },
            "required": ["pattern"]
        }
    },
    {
        "name": "list_files",
        "description": "Lists files in a directory. Use this to explore the project structure.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to list. Defaults to the project root."
                },
                "pattern": {
                    "type": "string",
                    "description": "Optional glob pattern to filter results (e.g. '*.py', '**/*.tsx')."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Optional. If true, list files recursively up to a limit. Useful to see structure of nested directories in one go."
                }
            },
            "required": []
        }
    },
    {
        "name": "view_signatures",
        "description": "Extracts class and function signatures and docstrings from a Python file. Use this to quickly understand a file's API structure without reading its full content, saving tokens.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute or relative path to the Python (.py) file to extract signatures from."
                }
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "run_js",
        "description": "Run Javascript code in an in-memory QuickJS sandbox (no Node.js) with a strict timeout to prevent infinite loops.",
        "parameters": {
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "The JavaScript code to execute. Max 20KB."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Optional. Timeout in milliseconds. Default 500."
                }
            },
            "required": ["code"]
        }
    },
    {
        "name": "query_history",
        "description": "Query past conversation context: pass query= to SEARCH past checkpoints/threads (semantic + keyword), or checkpoint_id= to READ a specific archived conversation log from /conversation_history/.",
        "parameters": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural-language search over past sessions ('how did we fix the cart bug last time?')."
                },
                "checkpoint_id": {
                    "type": "string",
                    "description": "Filename of an archived conversation log to read (e.g. 'chat_abc.jsonl')."
                }
            },
            "required": []
        }
    },
    {
        "name": "web_fetch",
        "description": "Fetch a URL via plain HTTP GET and return text content. Fast â€” no browser overhead. Use for documentation, API references, blog posts, static pages. NOT for JavaScript-rendered SPAs or login-required pages.",
        "parameters": {
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The full URL to fetch (https://...)."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Optional. Maximum characters to return (default 10000)."
                }
            },
            "required": ["url"]
        }
    },
    {
        "name": "browser_navigate",
        "description": "Open a URL in a headless Chromium browser and return the page content as an accessibility snapshot with @ref selectors. Handles JavaScript-rendered pages, SPAs, complex web apps. Returns interactive element tree for use with other browser tools. Slower than web_fetch but handles everything.",
        "parameters": {
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The full URL to navigate to."
                },
                "wait_ms": {
                    "type": "integer",
                    "description": "Optional. Milliseconds to wait for page load (default 5000)."
                }
            },
            "required": ["url"]
        }
    },
    {
        "name": "browser_extract",
        "description": "Extract content from the current browser page. Use after browser_navigate. Can extract text, HTML, attribute values, page title, or URL. Uses @ref selectors from the accessibility snapshot.",
        "parameters": {
            "type": "object",
            "properties": {
                "what": {
                    "type": "string",
                    "description": "What to extract: 'text' (default), 'html', 'value', 'title', 'url', 'attr name'."
                },
                "selector": {
                    "type": "string",
                    "description": "Optional CSS selector or @ref from snapshot. Empty = entire page."
                }
            },
            "required": []
        }
    },
    {
        "name": "browser_screenshot",
        "description": "Take a screenshot of the current browser page. Saves to a temp file and returns the path. Use for visual verification of pages.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional file path to save the screenshot. Auto-named if empty."
                }
            },
            "required": []
        }
    },
    {
        "name": "browser_close",
        "description": "Close the current browser session and free all resources (memory, Chrome process). Call when done with web research to avoid resource leaks.",
        "parameters": {
            "type": "object",
            "properties": {},
            "required": []
        }
    },
    {
        "name": "update_feature",
        "description": "Add or update a feature entry in long-term project memory (features.md) after a feature is implemented. file_refs is a list of {\"path\", \"lines\", \"purpose\"} objects.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Feature name, e.g. 'User Profile Page'."},
                "description": {"type": "string", "description": "Short description of the feature."},
                "file_refs": {"type": "array", "description": "List of {\"path\", \"lines\", \"purpose\"} file references."}
            },
            "required": ["name"]
        }
    },
    {
        "name": "update_project_meta",
        "description": "Add/update a project metadata section in long-term memory (project.md). Only these keys are allowed: Tech Stack, Frontend, Database, Testing, Architecture, Project Type, Goals.",
        "parameters": {
            "type": "object",
            "properties": {
                "key": {"type": "string", "description": "Metadata key (Tech Stack, Frontend, Database, Testing, Architecture, Project Type, Goals)."},
                "value": {"type": "string", "description": "Metadata value."}
            },
            "required": ["key", "value"]
        }
    },
]


# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
# Tool Implementations
# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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


def _check_and_request_approval(file_path: str, action: str):
    import os
    norm_path = os.path.normpath(file_path).lower()
    base_name = os.path.basename(norm_path)

    # Core settings files should only be blocked if they are modified inside the main LangChain codebase folder.
    # Subagents executing inside sandbox workspaces (e.g. TestProjectForAgent) must be allowed to create/edit package.json, etc.
    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws_norm = os.path.normpath(active_ws).lower()
    # Harness root — env-overridable, defaults to this repo's directory (portable)
    langchain_root = os.environ.get("BSA_HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))
    langchain_root = os.path.normpath(langchain_root).lower()
    
    in_langchain = (
        norm_path.startswith(langchain_root)
        or (not norm_path.startswith("c:") and not norm_path.startswith("d:")) # Relative paths are usually inside active project
        and langchain_root in active_ws_norm
    )

    is_core_config = in_langchain and (
        ".deep_agents" in norm_path
        or ".antigravity" in norm_path
        or base_name in [
            "settings.json", "user_profile.json", "rules.json",
            "requirements.txt", "package.json", "setup.py",
            "tsconfig.json", "vite.config.ts", "next.config.js", "next.config.mjs"
        ]
        or base_name.endswith((".json", ".toml", ".yaml", ".yml"))
    )
    
    if is_core_config:
        raise PermissionError(f"Modification to core configuration file '{file_path}' is blocked without human approval.")

def write_file(file_path: str, content: str) -> str:
    """Write content to a file, creating parent directories if needed."""
    enforce_fs_permission("write", file_path)
    _increment_cache_version()
    _check_and_request_approval(file_path, "write")

    # â”€â”€ Resolve the actual write target â”€â”€
    norm_path = os.path.normpath(os.path.abspath(file_path))

    # Direct filesystem write for absolute paths within the active workspace.
    # Bypasses VFS routing which strips path prefixes and loses the root directory.
    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws = os.path.normpath(os.path.abspath(active_ws))
    if norm_path.lower().startswith(active_ws.lower()):
        os.makedirs(os.path.dirname(norm_path) or ".", exist_ok=True)
        try:
            _snapshot_file(norm_path)  # C11: undo primitive
            with open(norm_path, "w", encoding="utf-8") as f:
                f.write(content)
            _invalidate_file_cache(norm_path)
            lines = content.count("\n") + 1
            syntax_err = _validate_syntax(norm_path)
            if syntax_err:
                output = f"[WARNING] Wrote {file_path} ({lines} lines, {len(content)} chars) but syntax checks failed:\n{syntax_err}"
            else:
                output = f"[OK] Wrote {file_path} ({lines} lines, {len(content)} chars)"
            _log_tool("write_file", {"file_path": file_path, "content": content}, output)
            return output
        except Exception as e:
            return f"Error writing {file_path}: {e}"

    # â”€â”€ VFS Composite Routing (for virtual paths like /workspace/, /scratch/) â”€â”€
    vfs_path = file_path.replace("\\", "/")
    try:
        vfs_router = get_vfs_router()
        res = vfs_router.write(vfs_path, content)
        if not res.error:
            lines = content.count("\n") + 1
            output = f"[OK] Wrote {file_path} ({lines} lines, {len(content)} chars)"
            _log_tool("write_file", {"file_path": file_path, "content": content}, output)
            return output
    except Exception:
        pass

    path = _sanitize_path(file_path)
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    try:
        _snapshot_file(path)  # C11: undo primitive
        with open(path, "w", encoding="utf-8") as f:
            f.write(content)
        _invalidate_file_cache(path)
        lines = content.count("\n") + 1
        
        # Validate syntax
        syntax_err = _validate_syntax(path)
        if syntax_err:
            output = f"[WARNING] Wrote {file_path} ({lines} lines, {len(content)} chars) successfully, but syntax checks failed:\n{syntax_err}"
        else:
            output = f"[OK] Wrote {file_path} ({lines} lines, {len(content)} chars)"
            
        _log_tool("write_file", {"file_path": file_path, "content": content}, output)
        return output
    except Exception as e:
        return f"Error writing {file_path}: {e}"


def edit_file(file_path: str, old_string: str = None, new_string: str = None, diff: str = None) -> str:
    """Edit a file via single search/replace OR batch SEARCH/REPLACE blocks."""
    enforce_fs_permission("write", file_path)
    _increment_cache_version()
    _check_and_request_approval(file_path, "edit")
    
    # Route to batch mode if diff is provided
    if diff:
        return apply_diff(file_path, diff)
        
    # Single edit mode â€” require both old_string and new_string
    if not old_string or new_string is None:
        return "Error: edit_file requires either (old_string + new_string) for single edit or (diff) for batch SEARCH/REPLACE blocks."
    if old_string == new_string:
        return "Error: old_string and new_string are identical."

    # â”€â”€ Direct filesystem edit for absolute workspace paths â”€â”€
    norm_path = os.path.normpath(os.path.abspath(file_path))
    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws = os.path.normpath(os.path.abspath(active_ws))
    if norm_path.lower().startswith(active_ws.lower()):
        path = norm_path
    else:
        # â”€â”€ VFS Composite Routing (for virtual paths) â”€â”€
        vfs_path = file_path.replace("\\", "/")
        try:
            vfs_router = get_vfs_router()
            res = vfs_router.edit(vfs_path, old_string, new_string)
            if not res.error:
                output = f"[OK] Edited {file_path} successfully"
                _log_tool("edit_file", {"file_path": file_path, "old_string": old_string, "new_string": new_string}, output)
                return output
        except Exception:
            pass
        path = _sanitize_path(file_path)
    if not os.path.isfile(path):
        return f"Error: File not found: {file_path}"

    try:
        with open(path, "r", encoding="utf-8") as f:
            content = f.read()
        if old_string not in content:
            # Try fuzzy match fallback: ignore trailing whitespace and normalize line endings
            normalized_content = content.replace('\r\n', '\n')
            normalized_old = old_string.replace('\r\n', '\n')
            normalized_new = new_string.replace('\r\n', '\n')
            
            old_lines = [line.rstrip() for line in normalized_old.split('\n')]
            content_lines = [line.rstrip() for line in normalized_content.split('\n')]
            
            match_index = -1
            match_count = 0
            n_old = len(old_lines)
            n_content = len(content_lines)
            
            for start_idx in range(n_content - n_old + 1):
                window = content_lines[start_idx : start_idx + n_old]
                if all(w == o for w, o in zip(window, old_lines)):
                    match_index = start_idx
                    match_count += 1
                    
            if match_count == 1:
                # We found exactly one match!
                orig_lines_all = normalized_content.split('\n')
                original_matched_lines = orig_lines_all[match_index : match_index + n_old]
                original_matched_block = '\n'.join(original_matched_lines)
                
                # Indentation adjustment
                first_matched_line = orig_lines_all[match_index]
                first_old_line = normalized_old.split('\n')[0] if normalized_old.split('\n') else ""
                
                matched_indent = len(first_matched_line) - len(first_matched_line.lstrip())
                old_indent = len(first_old_line) - len(first_old_line.lstrip())
                
                indent_diff = matched_indent - old_indent
                adjusted_new = normalized_new
                if indent_diff != 0:
                    new_lines_adjusted = []
                    for new_line in normalized_new.split('\n'):
                        if new_line.strip():
                            if indent_diff > 0:
                                new_lines_adjusted.append(' ' * indent_diff + new_line)
                            else:
                                leading_spaces = len(new_line) - len(new_line.lstrip())
                                strip_len = min(abs(indent_diff), leading_spaces)
                                new_lines_adjusted.append(new_line[strip_len:])
                        else:
                            new_lines_adjusted.append(new_line)
                    adjusted_new = '\n'.join(new_lines_adjusted)
                    
                new_content = normalized_content.replace(original_matched_block, adjusted_new)
                if '\r\n' in content:
                    new_content = new_content.replace('\n', '\r\n')
            else:
                if match_count > 1:
                    return f"Error: Fuzzy match found multiple matches ({match_count}) in {file_path}. Make old_string more specific."
                else:
                    # M1 edit-failure contract: give the model a precise on-disk
                    # anchor (line range + nearest snippet) so it can retry blind
                    # with exact text instead of guessing.
                    content_lines_all = normalized_content.split('\n')
                    total_lines = len(content_lines_all)
                    if match_index >= 0:
                        anchor_line_number = match_index + 1
                        anchor_line = max(0, match_index - 1)
                    else:
                        # no fuzzy match either — anchor at the file head
                        anchor_line_number = 1
                        anchor_line = 0
                    anchor_snippet = "\n".join(content_lines_all[anchor_line:anchor_line + 5])[:400]
                    return (
                        f"Error: old_string not found in {file_path}. The text must match exactly.\n"
                        f"[HINT] File has {total_lines} lines. Nearest anchor: line {anchor_line_number}. "
                        f"On-disk content around line {anchor_line_number}:\n"
                        f"```\n{anchor_snippet}\n```\n"
                        f"Read the file with read_file(file_path, offset={max(1, anchor_line_number - 10)}, limit=30) "
                        f"to get the exact lines, then retry with the verbatim text."
                    )
        else:
            if old_string == new_string:
                return f"Error: old_string and new_string are identical."
            count = content.count(old_string)
            if count > 1:
                return f"Error: old_string matches {count} times in {file_path}. To prevent unintended edits, please include more surrounding context lines in old_string to make it unique."
            new_content = content.replace(old_string, new_string, 1)  # Replace first occurrence only
        _snapshot_file(path)  # C11: undo primitive
        with open(path, "w", encoding="utf-8") as f:
            f.write(new_content)
        _invalidate_file_cache(path)
        
        # Validate syntax
        syntax_err = _validate_syntax(path)
        # Rich result: show the edited region (±10 lines) so the model never
        # needs to re-read the file to "see" its edit (the re-read loop killer).
        edit_note = _edit_snippet(file_path, content, new_content)
        if syntax_err:
            output = f"[WARNING] Edited {file_path} (1 replacement) successfully, but syntax checks failed:\n{syntax_err}"
        else:
            output = f"[OK] Edited {file_path} (1 replacement)\n{edit_note}"
            
        _log_tool("edit_file", {"file_path": file_path, "old_string": old_string, "new_string": new_string}, output)
        return output
    except Exception as e:
        return f"Error editing {file_path}: {e}"


def apply_search_replace(content: str, diff: str) -> tuple[str, list[str]]:
    # Normalize line endings to \n for internal processing
    normalized_content = content.replace('\r\n', '\n')
    normalized_diff = diff.replace('\r\n', '\n')
    
    pattern = re.compile(
        r'<<<<<<< SEARCH\n(.*?)\n=======\n(.*?)\n>>>>>>> REPLACE',
        re.DOTALL
    )
    blocks = pattern.findall(normalized_diff)
    if not blocks:
        return "", ["No SEARCH/REPLACE blocks found in diff. Use <<<<<<< SEARCH\\n[old]\\n=======\\n[new]\\n>>>>>>> REPLACE format."]
        
    errors = []
    for old, new in blocks:
        count = normalized_content.count(old)
        if count == 0:
            # Try fuzzy match fallback: ignore trailing whitespace and normalize line endings
            old_lines = [line.rstrip() for line in old.split('\n')]
            content_lines = [line.rstrip() for line in normalized_content.split('\n')]
            
            # Find sequence of lines in content_lines that match old_lines
            match_index = -1
            match_count = 0
            n_old = len(old_lines)
            n_content = len(content_lines)
            
            for start_idx in range(n_content - n_old + 1):
                window = content_lines[start_idx : start_idx + n_old]
                if all(w == o for w, o in zip(window, old_lines)):
                    match_index = start_idx
                    match_count += 1
                    
            if match_count == 1:
                # Reconstruct the matched block from the original content to replace it
                orig_lines_all = normalized_content.split('\n')
                original_matched_lines = orig_lines_all[match_index : match_index + n_old]
                original_matched_block = '\n'.join(original_matched_lines)
                
                # Indentation adjustment
                first_matched_line = orig_lines_all[match_index]
                first_old_line = old.split('\n')[0] if old.split('\n') else ""
                
                matched_indent = len(first_matched_line) - len(first_matched_line.lstrip())
                old_indent = len(first_old_line) - len(first_old_line.lstrip())
                
                indent_diff = matched_indent - old_indent
                adjusted_new = new
                if indent_diff != 0:
                    new_lines_adjusted = []
                    for new_line in new.split('\n'):
                        if new_line.strip():
                            if indent_diff > 0:
                                new_lines_adjusted.append(' ' * indent_diff + new_line)
                            else:
                                leading_spaces = len(new_line) - len(new_line.lstrip())
                                strip_len = min(abs(indent_diff), leading_spaces)
                                new_lines_adjusted.append(new_line[strip_len:])
                        else:
                            new_lines_adjusted.append(new_line)
                    adjusted_new = '\n'.join(new_lines_adjusted)
                    
                normalized_content = normalized_content.replace(original_matched_block, adjusted_new)
            else:
                if match_count > 1:
                    errors.append(f"Fuzzy match found multiple matches ({match_count}) in file. Please make the search block more specific.")
                else:
                    errors.append(f"Search block not found in file. Ensure exact match including indentation:\n{old}")
        elif count > 1:
            errors.append(f"Search block matches multiple times ({count}) in file. Make the search block more specific.")
        else:
            normalized_content = normalized_content.replace(old, new)
            
    if errors:
        return "", errors
        
    if '\r\n' in content:
        final_content = normalized_content.replace('\n', '\r\n')
    else:
        final_content = normalized_content
    return final_content, []


def apply_unified_diff(content: str, diff: str) -> tuple[str, list[str]]:
    lines = content.replace('\r\n', '\n').split('\n')
    diff_lines = diff.replace('\r\n', '\n').split('\n')
    
    hunks = []
    current_hunk = None
    
    for line in diff_lines:
        if line.startswith('@@'):
            if current_hunk:
                hunks.append(current_hunk)
            m = re.match(r'@@ -(\d+),?(\d+)? \+(\d+),?(\d+)? @@', line)
            if m:
                old_start = int(m.group(1))
                current_hunk = {'old_start': old_start, 'lines': []}
            else:
                current_hunk = None
        elif current_hunk is not None:
            if line.startswith('-') or line.startswith('+') or line.startswith(' '):
                current_hunk['lines'].append(line)
                
    if current_hunk:
        hunks.append(current_hunk)
        
    if not hunks:
        return "", ["No unified diff hunks found (missing @@ header or matching lines)."]
        
    hunks.sort(key=lambda h: h['old_start'], reverse=True)
    
    errors = []
    new_lines = list(lines)
    for hunk in hunks:
        start_idx = hunk['old_start'] - 1
        hunk_old_lines = [l[1:] for l in hunk['lines'] if l.startswith('-') or l.startswith(' ')]
        hunk_new_lines = [l[1:] for l in hunk['lines'] if l.startswith('+') or l.startswith(' ')]
        
        match = True
        for i, old_line in enumerate(hunk_old_lines):
            target_idx = start_idx + i
            if target_idx >= len(new_lines) or new_lines[target_idx] != old_line:
                match = False
                break
                
        if not match:
            found = False
            for offset in range(-10, 11):
                test_idx = start_idx + offset
                if test_idx < 0:
                    continue
                match = True
                for i, old_line in enumerate(hunk_old_lines):
                    target_idx = test_idx + i
                    if target_idx >= len(new_lines) or new_lines[target_idx] != old_line:
                        match = False
                        break
                if match:
                    start_idx = test_idx
                    found = True
                    break
            if not found:
                errors.append(f"Hunk starting at line {hunk['old_start']} failed to match context lines.")
                continue
                
        new_lines[start_idx : start_idx + len(hunk_old_lines)] = hunk_new_lines
        
    if errors:
        return "", errors
    return '\n'.join(new_lines), []


def apply_diff(file_path: str, diff: str) -> str:
    """Apply a unified diff or search-replace block to a file."""
    path = _sanitize_path(file_path)
    if not os.path.isfile(path):
        return f"Error: File not found: {file_path}"
        
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            content = f.read()
            
        if "<<<<<<< SEARCH" in diff:
            new_content, errors = apply_search_replace(content, diff)
        else:
            new_content, errors = apply_unified_diff(content, diff)
            
        if errors:
            return f"Error: Failed to apply diff to {file_path}:\n" + "\n".join(errors)

        _snapshot_file(path)  # C11: undo primitive
        with open(path, "w", encoding="utf-8") as f:
            f.write(new_content)
        _invalidate_file_cache(path)
            
        # Validate syntax
        syntax_err = _validate_syntax(path)
        lines = new_content.count("\n") + 1
        if syntax_err:
            output = f"[WARNING] Applied diff to {file_path} successfully, but syntax checks failed:\n{syntax_err}"
        else:
            output = f"[OK] Applied diff to {file_path} ({lines} lines)"
            
        _log_tool("apply_diff", {"file_path": file_path, "diff": diff}, output)
        return output
    except Exception as e:
        return f"Error applying diff to {file_path}: {e}"


BLOCKED_PATTERNS = {
    "rm -rf /", "rm -rf ~", "del /s /q c:\\", "format c:",
    "shutdown", "mkfs", "dd if=", ":(){",
    # C12: mechanical command policy — destructive / irreversible / system-level
    # PowerShell patterns are root-qualified so targeted cleanups like
    # `Remove-Item -Recurse -Force node_modules` or `Remove-Item -Force file`
    # still work (substring match; `-recurse -force .` only hits dot-prefixed
    # paths and explicit current-dir deletes).
    "git push", "git reset --hard", "git clean -f",
    "rm -rf .", "rm -r /", "rm -f /",
    "del /f /q /s", "rd /s /q", "rmdir /s /q",
    "remove-item -recurse -force .", "remove-item -recurse -force *",
    "remove-item -recurse -force c:", "remove-item -recurse -force d:",
    "net user", "net localgroup", "diskpart",
    "taskkill /f", "pkill -9", "kill -9",
    "reg delete", "reg add", "regedit",
    "chkdsk /f", "sfc /scannow", "format d:",
}

# C12: env-extensible deny list — comma-separated extra patterns (BSA_CMD_DENY_EXTRA)
def _load_extra_deny_patterns() -> set:
    extra = os.environ.get("BSA_CMD_DENY_EXTRA", "")
    if not extra:
        return set()
    return {p.strip().lower() for p in extra.split(",") if p.strip()}

BACKGROUND_PROCESSES = {}
_BACKGROUND_PROCS_LOCK = threading.Lock()

# cmd.exe mangles multiline `python -c "..."` (empty output, exit 0), so agents' python
# self-verification silently fails on Windows. Rewrite multiline -c blocks to a temp script.
_PY_C_RE = re.compile(r'(?<![A-Za-z0-9_.-])((?:python|python3)\s+-c\s+)"([\s\S]*)"', re.IGNORECASE)


def _rewrite_multiline_python_c(command: str) -> str:
    """Rewrite multiline `python -c "..."` / `python3 -c "..."` to run a temp script file.

    cmd.exe treats each line of a multi-line quoted -c argument as a separate command, so
    Dev's verification runs nothing and reports empty output. Writing the block to a .py
    file and invoking `python <file>` makes it actually execute. Single-line -c is left alone.
    """
    if not command or "-c" not in command:
        return command
    m = _PY_C_RE.search(command)
    if not m:
        return command
    code = m.group(2)
    if "\n" not in code and len(code) < 300:
        return command  # short single-line -c works fine through cmd
    try:
        import tempfile
        fd, script = tempfile.mkstemp(suffix=".py", prefix="agent_check_")
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(code.replace('\\"', '"'))
    except Exception:
        return command
    return command[: m.start(0)] + f'python "{script}"' + command[m.end(0):]


class LocalShellBackend:
    def __init__(self, root_dir: str):
        self.root_dir = root_dir

    def execute(self, command: str, timeout: int = 30000, background: bool = False, name: str = None) -> str:
        global BACKGROUND_PROCESSES
        import os
        import re
        import subprocess
        import threading
        import queue
        import time

        # Construct custom environment with prepended virtual environment PATH
        custom_env = os.environ.copy()
        venv_scripts = None
        for venv_name in ["venv312", "venv", ".venv"]:
            for folder in ["Scripts", "bin"]:
                candidate = os.path.join(self.root_dir, venv_name, folder)
                if os.path.isdir(candidate):
                    venv_scripts = candidate
                    break
            if venv_scripts:
                break
        if not venv_scripts:
            project_root = _HARNESS_ROOT
            for venv_name in ["venv312", "venv", ".venv"]:
                for folder in ["Scripts", "bin"]:
                    candidate = os.path.join(project_root, venv_name, folder)
                    if os.path.isdir(candidate):
                        venv_scripts = candidate
                        break
                if venv_scripts:
                    break
        if venv_scripts:
            path_key = "PATH"
            for k in list(custom_env.keys()):
                if k.upper() == "PATH":
                    path_key = k
            existing_path = custom_env.get(path_key, "")
            if existing_path:
                custom_env[path_key] = venv_scripts + os.path.pathsep + existing_path
            else:
                custom_env[path_key] = venv_scripts

        # cmd.exe mangles multiline `python -c "..."` on Windows -> rewrite to a temp script
        command = _rewrite_multiline_python_c(command)

        # Wrap command in Docker container if configured
        use_docker = os.environ.get("DEEP_AGENTS_USE_DOCKER", "false").lower() == "true"
        if use_docker:
            abs_root = os.path.abspath(self.root_dir)
            image = os.environ.get("DEEP_AGENTS_DOCKER_IMAGE", "python:3.10-slim")
            escaped_command = command.replace('\\', '\\\\')
            escaped_command = escaped_command.replace('`', '\\`')
            escaped_command = escaped_command.replace('$', '\\$')
            escaped_command = escaped_command.replace('"', '\\"')
            command = f'docker run --rm -v "{abs_root}:/workspace" -w /workspace {image} sh -c "{escaped_command}"'

        if background:
            try:
                proc = subprocess.Popen(
                    command,
                    shell=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    cwd=self.root_dir,
                    env=custom_env,
                    encoding="utf-8",
                    errors="replace",
                )
                
                info = {
                    "proc": proc,
                    "command": command,
                    "logs": collections.deque(maxlen=2000),
                }

                def _log_reader():
                    for line in proc.stdout:
                        info["logs"].append(line)
                        try:
                            from state_sync import safe_append_live_log
                            # `line` already carries its trailing newline
                            safe_append_live_log(f"[{name}] {line}", newline=False)
                        except Exception:
                            pass
                            
                t = threading.Thread(target=_log_reader, daemon=True)
                t.start()
                info["thread"] = t
                with _BACKGROUND_PROCS_LOCK:
                    BACKGROUND_PROCESSES[name] = info
                
                time.sleep(1.5)
                if proc.poll() is not None:
                    exit_code = proc.poll()
                    initial_logs = _clean_terminal_output("".join(info["logs"]))
                    output = f"[ERROR] Background process '{name}' failed to start immediately (Exit code {exit_code}).\nLogs:\n{initial_logs}"
                    with _BACKGROUND_PROCS_LOCK:
                        BACKGROUND_PROCESSES.pop(name, None)
                    _log_tool("run_command_bg_fail", {"command": command, "name": name}, output)
                    return output
                
                output = f"[OK] Started background process '{name}' (PID {proc.pid}).\nUse `run_command` with process_action='logs' and command='{name}' to view output."
                _log_tool("run_command_bg", {"command": command, "name": name}, output)
                return output
                
            except Exception as e:
                return f"Error starting background process: {e}"
        else:
            safe_append_live_log(f"\n[Terminal] Run: {command}")
            
            try:
                proc = subprocess.Popen(
                    command,
                    shell=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    cwd=self.root_dir,
                    env=custom_env,
                    encoding="utf-8",
                    errors="replace",
                )
                
                q = queue.Queue()
                
                def read_output(stream, q):
                    for line in stream:
                        q.put(line)
                    stream.close()
                    
                t = threading.Thread(target=read_output, args=(proc.stdout, q))
                t.daemon = True
                t.start()
                
                out_lines = []
                start_time = time.time()
                max_duration = timeout / 1000.0
                
                while True:
                    while not q.empty():
                        try:
                            line = q.get_nowait()
                            out_lines.append(line)
                            # `line` already carries its trailing newline
                            safe_append_live_log(line, newline=False)
                        except queue.Empty:
                            break
                    
                    if proc.poll() is not None:
                        t.join(timeout=1.0)
                        while not q.empty():
                            try:
                                line = q.get_nowait()
                                out_lines.append(line)
                                # `line` already carries its trailing newline
                                safe_append_live_log(line, newline=False)
                            except queue.Empty:
                                break
                        break
                    
                    if time.time() - start_time > max_duration:
                        proc.kill()
                        t.join(timeout=1.0)
                        while not q.empty():
                            try:
                                line = q.get_nowait()
                                out_lines.append(line)
                            except queue.Empty:
                                break
                        output = "".join(out_lines)
                        cleaned = _clean_terminal_output(output)
                        capped = auto_offload_result(cleaned, command, max_chars=_tool_cap("run_command_timeout"))
                        result_str = f"Error: Command timed out after {timeout}ms: {command}\nPartial Output:\n{capped}\n[TIMEOUT]"
                        safe_append_live_log("\n[TIMEOUT]")
                        _log_tool("run_command_timeout", {"command": command}, result_str[:500])
                        return result_str
                    
                    time.sleep(0.1)
                    
                exit_code = proc.wait()
                output = "".join(out_lines)
                cleaned = _clean_terminal_output(output)
                # Command/test output: head+tail matter (exit code, traceback, last
                # assertion); the middle is usually noise. Cap at ~6K chars (~1.5K tok)
                # so large test runs don't bloat the per-turn cache-miss input.
                capped = auto_offload_result(cleaned, command, max_chars=_tool_cap("run_command"))
                if exit_code != 0:
                    result_str = f"[ERR] Exit {exit_code}\n{capped}"
                else:
                    result_str = f"[OK] Exit 0\n{capped}"

                # Run-output dedup (C): re-running the same command with byte-identical
                # output (e.g. re-running a test suite between fixes) re-appends the same
                # text as fresh cache-miss input. If the exact output already appeared in
                # this session, return a stub — the transcript has it.
                try:
                    import hashlib as _hl
                    run_key = command.strip()
                    run_hash = _hl.md5(result_str.encode("utf-8", errors="replace")).hexdigest()
                    if run_key in _RUN_OUTPUT_CACHE and _RUN_OUTPUT_CACHE[run_key] == run_hash:
                        status = "OK" if exit_code == 0 else "ERR"
                        result_str = (f"[{status}] Exit {exit_code}\n"
                                      f"[REPEAT RUN — identical output to an earlier run of this command; "
                                      f"the full output is already in this conversation.]")
                    else:
                        _RUN_OUTPUT_CACHE[run_key] = run_hash
                except Exception:
                    pass

                _log_tool("run_command", {"command": command}, result_str[:500])
                return result_str
                
            except Exception as e:
                return f"Error running command: {e}"


def run_command(command: str = "", timeout: int = 30000, background: bool = False, process_action: str = "") -> str:
    """Execute a shell command, manage background processes, or view logs."""
    global BACKGROUND_PROCESSES
    import threading
    import re

    # If timeout is passed in seconds instead of milliseconds, scale it
    if timeout is not None and timeout < 1000:
        timeout = timeout * 1000

    # Clear local file cache since executing a command can modify files on disk
    _FILE_CACHE.clear()
    _increment_cache_version()

    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws = os.path.normpath(os.path.abspath(active_ws))

    # Safety validation (C12: BLOCKED_PATTERNS + env-extensible deny list)
    if command:
        cmd_lower = command.lower().strip()
        deny_patterns = BLOCKED_PATTERNS | _load_extra_deny_patterns()
        if any(p in cmd_lower for p in deny_patterns):
            matched = [p for p in deny_patterns if p in cmd_lower]
            return f"Error: Command execution blocked for safety. Dangerous pattern detected: {matched[0]}"

    # Handle process actions
    if process_action:
        if process_action == "list":
            with _BACKGROUND_PROCS_LOCK:
                # Filter out dead processes safely in-place without re-assigning dict
                dead_keys = [k for k, info in BACKGROUND_PROCESSES.items() if info["proc"].poll() is not None]
                for k in dead_keys:
                    BACKGROUND_PROCESSES.pop(k, None)
                
                if not BACKGROUND_PROCESSES:
                    return "No running background processes."
                lines = ["Active background processes:"]
                for name, info in BACKGROUND_PROCESSES.items():
                    lines.append(f"- {name}: '{info['command']}' (PID {info['proc'].pid})")
                return "\n".join(lines)
            
        elif process_action == "kill":
            name = command.strip()
            with _BACKGROUND_PROCS_LOCK:
                if not name or name not in BACKGROUND_PROCESSES:
                    return f"Error: No background process named '{name}' is currently running."
                info = BACKGROUND_PROCESSES[name]
            proc = info["proc"]
            try:
                proc.terminate()
                proc.wait(timeout=2)
                output = f"[OK] Process '{name}' (PID {proc.pid}) terminated."
            except Exception:
                try:
                    proc.kill()
                    proc.wait(timeout=2)
                    output = f"[WARNING] Process '{name}' (PID {proc.pid}) killed with force."
                except Exception as e:
                    output = f"Error terminating process: {e}"
            with _BACKGROUND_PROCS_LOCK:
                BACKGROUND_PROCESSES.pop(name, None)
            return output
            
        elif process_action == "logs":
            name = command.strip()
            with _BACKGROUND_PROCS_LOCK:
                if not name or name not in BACKGROUND_PROCESSES:
                    return f"Error: No background process named '{name}'."
                info = BACKGROUND_PROCESSES[name]
                logs = "".join(info["logs"])
            if not logs:
                return f"Process '{name}' (PID {info['proc'].pid}) has produced no output yet."
            cleaned_logs = _clean_terminal_output(logs)
            capped_logs = auto_offload_result(cleaned_logs, f"logs_{name}", max_chars=_tool_cap("logs"))
            return f"--- LOGS FOR '{name}' ---\n{capped_logs}"
            
        else:
            return f"Error: Invalid process_action '{process_action}'."
            
    if not command:
        return "Error: command parameter is required."

    # Generate a unique key for background process
    name = None
    if background:
        clean_name = re.sub(r'[^a-zA-Z0-9_-]', '_', command.split()[0])
        with _BACKGROUND_PROCS_LOCK:
            name = clean_name
            suffix = 1
            while name in BACKGROUND_PROCESSES:
                name = f"{clean_name}_{suffix}"
                suffix += 1

    backend = LocalShellBackend(root_dir=active_ws)
    result = backend.execute(command, timeout=timeout, background=background, name=name)
    if isinstance(result, str):
        return _clean_terminal_output(result)
    return result


def search_code(pattern: str, path: str = "", glob: str = "") -> str:
    """Search file contents using Python regex (ripgrep-style patterns)."""
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
                    from repo_map_generator import get_file_signatures
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


def list_files(path: str = "", pattern: str = "", recursive: bool = False) -> str:
    """List files in a directory, optionally filtered by glob pattern. Excludes package, cache, and system directories."""
    active_ws = shared_state.get("project_path") or WORKSPACE
    enforce_fs_permission("read", path if path else active_ws)
    search_dir = _sanitize_path(path) if path else _sanitize_path(active_ws)
    if not os.path.isdir(search_dir):
        return f"Error: Directory not found: {path or active_ws}"

    from state_sync import active_store
    store = active_store.get()
    norm_dir = os.path.normpath(search_dir).lower().replace("\\", "/")
    cache_key = f"list_files:{norm_dir}:{pattern}:{recursive}"
    if store is not None:
        try:
            version = _get_cache_version()
            cached_item = store.get(("tool_cache",), cache_key)
            if cached_item is not None and isinstance(cached_item.value, dict):
                if cached_item.value.get("version") == version:
                    output = cached_item.value.get("result")
                    _log_tool("list_files", {"path": path, "pattern": pattern, "recursive": recursive, "cached": True}, output[:500])
                    return output
        except Exception:
            pass

    ignored_dirs = {
        "node_modules", "venv", ".venv", "venv312", ".git", "__pycache__",
        ".claude", ".antigravity", ".deep_agents", "scratch", "vendor",
        "dist", "build", ".next", ".vscode", ".idea", ".pytest_cache", ".mypy_cache"
    }
    results = []
    if pattern:
        full_pattern = os.path.join(search_dir, pattern)
        matches = globmod.glob(full_pattern, recursive=True)
        for m in sorted(matches):
            rel = os.path.relpath(m, search_dir)
            # Check if any part of the path is in ignored directories
            path_parts = set(rel.replace("\\", "/").split("/"))
            if path_parts.intersection(ignored_dirs):
                continue
            is_dir = "[D]" if os.path.isdir(m) else "[F]"
            size = f" ({os.path.getsize(m):,}b)" if os.path.isfile(m) else ""
            results.append(f"  {is_dir} {rel}{size}")
        output = f"{search_dir} [{pattern}]\n" + "\n".join(results[:100]) if results else f"No files matching '{pattern}' in {search_dir}"
        if len(output) > 2000:
            output = output[:2000] + "\n... (listing truncated)"
    elif recursive:
        try:
            for root, dirs, files in os.walk(search_dir):
                dirs[:] = [d for d in dirs if d not in ignored_dirs]
                rel_root = os.path.relpath(root, search_dir)
                if rel_root == ".":
                    rel_root = ""
                for d in sorted(dirs):
                    rel_path = os.path.join(rel_root, d).replace("\\", "/")
                    results.append(f"  [D] {rel_path}/")
                for f in sorted(files):
                    full_file = os.path.join(root, f)
                    rel_path = os.path.join(rel_root, f).replace("\\", "/")
                    size = f" ({os.path.getsize(full_file):,}b)" if os.path.isfile(full_file) else ""
                    results.append(f"  [F] {rel_path}{size}")
                # Cap the recursive listing at 150 items to avoid token bloat
                if len(results) >= 150:
                    results = results[:150]
                    results.append("  ... (capping recursive list at 150 entries)")
                    break
            output = f"{search_dir}/ (recursive)\n" + "\n".join(results)
            if len(output) > 2000:
                output = output[:2000] + "\n... (listing truncated)"
        except Exception as e:
            return f"Error: {e}"
    else:
        try:
            entries = sorted(os.listdir(search_dir))
            for entry in entries:
                if entry in ignored_dirs:
                    continue
                full = os.path.join(search_dir, entry)
                is_dir = "[D]" if os.path.isdir(full) else "[F]"
                size = f" ({os.path.getsize(full):,}b)" if os.path.isfile(full) else ""
                results.append(f"  [F] {entry}{size}" if is_dir == "[F]" else f"  [D] {entry}/")
            output = f"{search_dir}/\n" + "\n".join(results[:100])
        except PermissionError:
            return f"Error: Permission denied: {search_dir}"

    if store is not None:
        try:
            version = _get_cache_version()
            store.put(("tool_cache",), cache_key, {"version": version, "result": output})
        except Exception:
            pass

    _log_tool("list_files", {"path": path, "pattern": pattern, "recursive": recursive}, output[:500])
    return output


def view_signatures(file_path: str) -> str:
    """Extract class/function/method signatures and docstrings from Python, PHP, TS, JS, or Dart files."""
    path = _sanitize_path(file_path)
    if not os.path.isfile(path):
        return f"Error: File not found: {file_path}"
        
    _, ext = os.path.splitext(file_path)
    ext = ext.lower()
    supported_exts = {".py", ".php", ".ts", ".tsx", ".js", ".jsx", ".dart"}
    if ext not in supported_exts:
        return f"Error: view_signatures only supports: {', '.join(sorted(supported_exts))} files."
        
    if ext == ".py":
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as f:
                content = f.read()
            import ast
            tree = ast.parse(content)
            
            class PythonSignatureVisitor(ast.NodeVisitor):
                def __init__(self):
                    self.indent = 0
                    self.output = []
                
                def visit_ClassDef(self, node):
                    doc = ast.get_docstring(node)
                    doc_str = f'  # {doc.splitlines()[0]}' if doc and doc.splitlines() else ""
                    self.output.append("    " * self.indent + f"{node.lineno}: class {node.name}:{doc_str}")
                    self.indent += 1
                    self.generic_visit(node)
                    self.indent -= 1
                    
                def visit_FunctionDef(self, node):
                    self._visit_func(node)
                    
                def visit_AsyncFunctionDef(self, node):
                    self._visit_func(node, is_async=True)
                    
                def _visit_func(self, node, is_async=False):
                    args_list = []
                    for arg in node.args.args:
                        args_list.append(arg.arg)
                    if node.args.vararg:
                        args_list.append(f"*{node.args.vararg.arg}")
                    if node.args.kwarg:
                        args_list.append(f"**{node.args.kwarg.arg}")
                    prefix = "async def " if is_async else "def "
                    doc = ast.get_docstring(node)
                    doc_str = f'  # {doc.splitlines()[0]}' if doc and doc.splitlines() else ""
                    self.output.append("    " * self.indent + f"{node.lineno}: {prefix}{node.name}({', '.join(args_list)}):{doc_str}")
            
            visitor = PythonSignatureVisitor()
            visitor.visit(tree)
            if not visitor.output:
                return f"[SIGNATURES] {file_path} contains no class or function definitions."
            return f"[SIGNATURES] {file_path}:\n" + "\n".join(visitor.output)
        except Exception as e:
            return f"Error extracting signatures from {file_path}: {e}"
    else:
        try:
            from repo_map_generator import get_file_signatures_with_lines
            signatures = get_file_signatures_with_lines(path, ext)
            if not signatures:
                return f"[SIGNATURES] {file_path} contains no class or function definitions."
            
            lines = []
            for sig, depth, doc, line_no in signatures:
                indent = "    " * depth
                if doc:
                    lines.append(f"{line_no}: {indent}{sig}  # {doc}")
                else:
                    lines.append(f"{line_no}: {indent}{sig}")
            return f"[SIGNATURES] {file_path}:\n" + "\n".join(lines)
        except Exception as e:
            return f"Error extracting signatures from {file_path}: {e}"


def run_js(code: str, timeout_ms: int = 500) -> str:
    from js_interpreter import run_js_in_sandbox
    return run_js_in_sandbox(code, timeout_ms, extra_funcs)


def query_history(query: str = "", checkpoint_id: str = "") -> str:
    """
    Query past conversation context - one tool, two modes:

    - checkpoint_id=... -> READ a specific archived conversation log from
      /conversation_history/ (e.g. "chat_abc.jsonl").
    - query=... -> SEARCH past checkpoints/threads: semantic vector search on
      the "past_fixes" collection first, keyword fallback over checkpoints.db.

    Use sparingly - only when you need context from earlier sessions.
    """
    # Mode 1: read a specific archived conversation
    if checkpoint_id:
        path = checkpoint_id
        if not path.startswith("/conversation_history/"):
            path = f"/conversation_history/{os.path.basename(path)}"
        return read_file(path)

    # Mode 2: search past conversations
    import sqlite3
    import json
    import gzip

    if not query.strip():
        return "Error: provide query= (search past sessions) or checkpoint_id= (read one archive)."

    # Semantic vector search first
    try:
        from embedding_service import vector_search as vs, vector_count, vector_store as vs_store
        count = vector_count("past_fixes")
        if count > 0:
            semantic_results = vs("past_fixes", query, top_k=5, threshold=0.55)
            if semantic_results:
                lines = [f"Semantic Search Results (found {len(semantic_results)} match(es), searched {count} entries):"]
                for i, r in enumerate(semantic_results, 1):
                    meta = r.get("metadata", {})
                    src_m = meta.get("source", "unknown")
                    step = meta.get("step", "unknown")
                    request = meta.get("client_request", "")[:120]
                    lines.append(
                        f"  {i}. [{r['score']:.2f}] Thread={r['id'][:20]}... "
                        f"| Step={step} | Source={src_m}"
                    )
                    if request:
                        lines.append(f"     Request: {request}")
                return "\n".join(lines)
    except ImportError:
        pass
    except Exception as e:
        # Log and fall through to keyword search
        import sys
        print(f"[query_history] Vector search failed: {e}", file=sys.stderr)

    # Keyword fallback (original behavior)
    db_path = os.path.join(_HARNESS_ROOT, ".deep_agents", "checkpoints.db")
    if not os.path.isfile(db_path):
        return "No checkpoints database found."

    try:
        conn = sqlite3.connect(db_path)
        cur = conn.cursor()

        # Select all checkpoints ordered by thread and id
        cur.execute("SELECT thread_id, checkpoint_id, type, checkpoint, metadata FROM checkpoints ORDER BY thread_id, checkpoint_id DESC")
        rows = cur.fetchall()

        matches = []
        seen_threads = set()

        for thread_id, checkpoint_id, type_col, checkpoint_blob, metadata_blob in rows:
            # We want to search for keywords in metadata (contains user request, steps, etc.)
            # Decompress if compressed
            is_gzip = type_col and "+gzip" in type_col

            # Decompress metadata
            metadata_str = ""
            if metadata_blob:
                try:
                    if is_gzip:
                        metadata_str = gzip.decompress(metadata_blob).decode("utf-8", errors="replace")
                    else:
                        metadata_str = metadata_blob.decode("utf-8", errors="replace")
                except Exception:
                    metadata_str = str(metadata_blob)

            if not metadata_str:
                continue

            # Check if query matches metadata or thread_id
            if query.lower() in metadata_str.lower() or query.lower() in thread_id.lower():
                try:
                    meta_dict = json.loads(metadata_str)
                except Exception:
                    meta_dict = {"raw": metadata_str}

                # Format match summary
                source = meta_dict.get("source", "unknown")
                step = meta_dict.get("step", "unknown")
                client_req = meta_dict.get("client_request", "")
                if not client_req and "configurable" in meta_dict:
                    client_req = meta_dict.get("configurable", {}).get("client_request", "")

                matches.append(
                    f"- Thread: {thread_id} | Checkpoint: {checkpoint_id} | Step: {step} | Source: {source}\n"
                    f"  Metadata: {json.dumps(meta_dict, ensure_ascii=True)[:400]}..."
                )

                seen_threads.add(thread_id)
                if len(matches) >= 10:
                    break

        conn.close()

        if not matches:
            return f"No checkpoints matched query '{query}'."

        return "Search Results:\n" + "\n".join(matches)

    except Exception as e:
        return f"Error searching checkpoints: {e}"


def search_semantic_checkpoints(query: str = "", top_k: int = 5) -> str:
    """
    Pillar 76: Search checkpoint history by semantic meaning.
    Finds relevant past states without loading the full history.
    Example: "how did I fix database connection errors last time?"
    """
    if not query.strip():
        return "Error: Please provide a query describing what you're looking for."
    try:
        from embedding_service import vector_search
        results = vector_search("checkpoints", query, top_k=top_k, threshold=0.55)
    except ImportError:
        return "Semantic search unavailable (embedding_service not installed)."
    except Exception as e:
        return f"Semantic search error: {e}"

    if not results:
        return f"No semantically similar checkpoints found for: '{query[:200]}'"

    lines = [f"Semantic Checkpoint Search Results (top {len(results)}):"]
    for i, r in enumerate(results, 1):
        meta = r.get("metadata", {})
        phase = meta.get("phase", "unknown")
        step = meta.get("step", "unknown")
        summary = meta.get("summary", "")[:150]
        lines.append(
            f"  {i}. [{r['score']:.2f}] Phase={phase} | Step={step} | "
            f"CP={r['id'][:16]}..."
        )
        if summary:
            lines.append(f"     {summary}")
    return "\n".join(lines)


def _search_codebase_keyword_fallback(query: str, active_ws: str) -> str:
    """Keyword-search fallback for search_codebase when the embedding model is
    not present on disk. Keeps the tool useful (never a hard error) and avoids
    triggering a slow model download from inside an agent turn."""
    try:
        keywords = [w.strip() for w in re.split(r"\W+", query.lower()) if len(w) >= 3]
        pattern = "|".join(re.escape(k) for k in keywords[:6])
        if not pattern:
            return (f"Semantic search unavailable (embedding model missing) and "
                    f"no searchable keywords in query: '{query[:100]}'")
        raw = search_code(pattern, path=active_ws)
        return (
            f"Embedding model unavailable (bge-large-en-v1.5 not downloaded) â€” "
            f"fell back to keyword search.\n{raw}"
        )[:4000]
    except Exception as e2:
        return f"Semantic search error: {e2}"


def search_codebase(query: str = "", top_k: int = 6, language: str = "",
                    reindex: bool = False) -> str:
    """
    Semantic codebase search â€” find code by meaning, not exact text.
    Searches indexed functions, classes, methods, and documentation across the workspace.

    Pillar 77: Adjacent chunks from the same file are deduplicated.
    Pillar 86: Results are reranked by embedding similarity for final ordering.

    Args:
        query: Natural language description of what you're looking for.
               Examples: "authentication logic", "database connection pool",
               "error handling middleware", "user registration flow"
        top_k: Max results to return (default 6, max 20).
        language: Filter by language extension, e.g. "py", "ts", "php". Empty = all.
        reindex: If True, force a full re-index before searching.

    Returns:
        Formatted search results with file paths, line numbers, signatures, and scores.
    """
    if not query.strip():
        return "Error: Please provide a query describing what code you're looking for."

    # Auto-index if codebase collection is empty
    # Collection is namespaced per workspace (hash of project path) so searching
    # always hits THIS project's index â€” never a global/shared one from another
    # workspace or the harness's own repo.
    import hashlib
    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws = os.path.normpath(os.path.abspath(active_ws))
    collection = "codebase_" + hashlib.sha1(active_ws.lower().encode()).hexdigest()[:12]

    def _embedding_model_available() -> bool:
        """Cheap check: does a usable bge-large-en-v1.5 model.onnx exist on disk?
        Avoids triggering a 1.3GB fastembed download when it's absent."""
        try:
            cache_root = os.environ.get(
                "FASTEMBED_CACHE_PATH",
                os.path.join(_HARNESS_ROOT, ".deep_agents", "fastembed_cache"),
            )
            repo = os.path.join(cache_root, "models--qdrant--bge-large-en-v1.5-onnx", "blobs")
            if not os.path.isdir(repo):
                return False
            for fn in os.listdir(repo):
                try:
                    if os.path.getsize(os.path.join(repo, fn)) > 50_000_000:
                        return True
                except OSError:
                    continue
            return False
        except Exception:
            return False

    if not _embedding_model_available():
        # Model not present â€” skip indexing + vector search entirely (they would
        # trigger a slow download) and degrade to keyword search.
        return _search_codebase_keyword_fallback(query, active_ws)

    try:
        from embedding_service import vector_count
        if reindex or vector_count(collection) == 0:
            from codebase_indexer import index_codebase
            stats = index_codebase(active_ws, force=reindex, collection=collection)
            if stats["files_indexed"] > 0:
                print(f"[search_codebase] Indexed {stats['files_indexed']} files "
                      f"({stats['chunks_created']} chunks) in {stats['time_seconds']}s")
    except ImportError:
        return "Codebase indexer not available. Please ensure codebase_indexer.py exists."
    except Exception as e:
        return f"Error during codebase indexing: {e}"

    # â”€â”€ Step 1: Vector search â”€â”€
    try:
        from embedding_service import vector_search as vs, embed_cached
        # Use cached embedding for the query (queries repeat often â€” Pillar 117)
        results = vs(collection, query, top_k=min(top_k * 2, 20), threshold=0.45)
    except Exception as e:
        return f"Semantic search error: {e}"

    if not results:
        return (f"No code found semantically matching: '{query[:200]}'\n"
                f"Try re-indexing with search_codebase(reindex=True) "
                f"or use search_code() for exact pattern matching.")

    # â”€â”€ Pillar 77: Dedup adjacent chunks from same file â”€â”€
    results = _dedup_search_results(results, max_line_gap=50)

    # â”€â”€ Pillar 86: Rerank â”€â”€
    # Re-embed query against each candidate's full search text for precise ranking
    if len(results) > 1:
        try:
            from embedding_service import embed_cached, cosine_similarity
            query_vec = embed_cached(query)
            for r in results:
                # Recompute similarity with full metadata context
                meta = r.get("metadata", {})
                full_text = (
                    f"{meta.get('kind', '')} {meta.get('symbol', '')}: "
                    f"{meta.get('signature', '')} â€” {meta.get('docstring', '')}"
                )[:1000]
                if full_text.strip():
                    from embedding_service import embed_cached, cosine_similarity
                    full_vec = embed_cached(full_text)
                    r["score"] = round(cosine_similarity(query_vec, full_vec), 4)
            # Re-sort by reranked score
            results.sort(key=lambda x: x["score"], reverse=True)
        except Exception:
            pass  # If reranking fails, keep original scores

    # â”€â”€ Filter by language â”€â”€
    if language:
        results = [r for r in results
                   if r.get("metadata", {}).get("language", "") == language.lower()]

    results = results[:top_k]

    # â”€â”€ Format output â”€â”€
    lines = [f"Semantic Codebase Search â€” '{query[:150]}' â€” top {len(results)} results:"]
    for i, r in enumerate(results, 1):
        meta = r.get("metadata", {})
        fp = meta.get("file_path", "?")
        symbol = meta.get("symbol", "?")
        line = meta.get("line", "?")
        kind = meta.get("kind", "?")
        sig = meta.get("signature", "")
        doc = meta.get("docstring", "")

        # Visual indicator for match quality
        bar = _score_bar(r["score"])

        lines.append(f"\n  {i}. {bar} [{r['score']:.2f}] {fp}:{symbol}() â€” line {line}")
        if sig:
            lines.append(f"     {sig[:120]}")
        if doc:
            lines.append(f'     "{doc[:120]}"')

    return "\n".join(lines)

def _score_bar(score: float) -> str:
    """Visual score indicator for search results."""
    if score >= 0.85:
        return "ðŸŸ¢"
    elif score >= 0.70:
        return "ðŸŸ¡"
    elif score >= 0.55:
        return "ðŸŸ "
    else:
        return "ðŸ”´"


def _dedup_search_results(results: list[dict], max_line_gap: int = 50) -> list[dict]:
    """
    Pillar 77: Merge search results from the same file with adjacent line ranges.
    Keeps the higher-scoring chunk when two are within max_line_gap lines.
    """
    if len(results) <= 1:
        return results

    # Group by file_path
    by_file: dict[str, list[dict]] = {}
    for r in results:
        fp = r.get("metadata", {}).get("file_path", "")
        by_file.setdefault(fp, []).append(r)

    deduped = []
    for fp, file_results in by_file.items():
        # Sort by line number
        file_results.sort(key=lambda r: r.get("metadata", {}).get("line", 0))
        kept = []
        for r in file_results:
            if not kept:
                kept.append(r)
                continue
            last = kept[-1]
            gap = abs(
                r.get("metadata", {}).get("line", 0) -
                last.get("metadata", {}).get("line", 0)
            )
            if gap <= max_line_gap:
                # Keep the higher-scoring one, add note to the kept one
                if r["score"] > last["score"]:
                    # Replace last if new one scores higher
                    r["metadata"]["_merged_from"] = last.get("id", "")
                    kept[-1] = r
                # else: skip this one (last is better or equal)
            else:
                kept.append(r)
        deduped.extend(kept)

    return deduped


class SubagentsRegistry(dict):
    def __contains__(self, key):
        if super().__contains__(key):
            return True
        if isinstance(key, str) and "-" in key:
            prefix = key.split("-")[0]
            if prefix in self:
                return True
        return False

    def __getitem__(self, key):
        if super().__contains__(key):
            return super().__getitem__(key)
        if isinstance(key, str) and "-" in key:
            prefix = key.split("-")[0]
            if prefix in self:
                val = dict(super().__getitem__(prefix))
                val["name"] = key
                return val
        raise KeyError(key)

    def get(self, key, default=None):
        try:
            return self[key]
        except KeyError:
            return default

_SUBAGENTS_REGISTRY = SubagentsRegistry()

# â”€â”€ Async Task Registry â”€â”€
import threading
from concurrent.futures import ThreadPoolExecutor
import contextvars

_ASYNC_TASKS: dict[str, dict] = {}
_ASYNC_LOCK = threading.Lock()
_ASYNC_EXECUTOR = ThreadPoolExecutor(max_workers=20)
_TASK_DEPTH: contextvars.ContextVar[int] = contextvars.ContextVar("task_depth", default=0)
_MAX_TASK_DEPTH = 3

# Tools added to every subagent for recursive delegation
_DELEGATION_TOOLS = ["task", "start_async_task", "check_async_task", "list_async_tasks"]


def _load_subagents():
    global _SUBAGENTS_REGISTRY
    try:
        from bsa_prompt import _STATIC_SYSTEM_TEMPLATE as _BSA_SYSTEM_TEMPLATE
        from devops_prompt import _STATIC_SYSTEM_TEMPLATE as _DEVOPS_SYSTEM_TEMPLATE
        from developer_prompt import _STATIC_SYSTEM_TEMPLATE as _DEV_SYSTEM_TEMPLATE
        from sdet_prompt import _STATIC_SYSTEM_TEMPLATE as _SDET_SYSTEM_TEMPLATE
        from free_agent_prompt import _FREE_AGENT_SYSTEM_TEMPLATE

        subs = [
            {
                "name": "Dev",
                "description": "Developer â€” pure implementer: receives BSA specs, writes clean tested TDD code, handles errors",
                "system_prompt": _DEV_SYSTEM_TEMPLATE,
                "tools": [
                    "read_file", "write_file", "edit_file", "run_command",
                    "search_code", "list_files", "revert_last_change",
                    "search_codebase", "view_signatures", "web_fetch",
                    "browser_act",
                ],
                "model": "deepseek:v4-flash",
            },
            {
                "name": "BSA",
                "description": "Business Systems Analyst â€” adaptive BA+SA: business analysis (BRD, Gherkin), systems architecture (schemas, API contracts, layered design), runs only the phases needed per task",
                "system_prompt": _BSA_SYSTEM_TEMPLATE,
                "tools": ["read_file", "write_file", "edit_file", "search_code",
                          "list_files", "view_signatures", "search_codebase",
                          "web_fetch", "run_command"],
                "model": "deepseek:v4-flash",
                "permissions": [
                    {"operations": ["write_file", "edit_file"],
                     "paths": [
                         "/workspace/specs/**",
                         "/workspace/docs/**",
                         "/workspace/DECISION-*",
                         "/workspace/SPEC-*",
                         "/workspace/README*"
                     ],
                     "mode": "allow"}
                ],
            },
            {
                "name": "DevSecOps",
                "description": "DevSecOps â€” application-layer security + performance hardening: SAST/SCA, OWASP application security, secrets hygiene, dependency audits, code-level vulnerability fixes, query/caching/rendering performance, incident response, Nginx hardening when present",
                "system_prompt": _DEVOPS_SYSTEM_TEMPLATE,
                "tools": ["run_command", "write_file", "edit_file", "search_code",
                          "read_file", "list_files", "search_codebase", "view_signatures"],
                "model": "deepseek:v4-flash",
                "reasoning_effort": "max",
            },
            {
                "name": "SDET",
                "description": "Software Developer in Test — the harness's FIXER: diagnoses failing verification (test output, hidden-test failure, harness gate output), finds the ROOT CAUSE, fixes it directly with the full Dev toolset, and re-runs verification until it passes. Prefers the project's own test framework. Handles bug-fix and verification requests. Does NOT audit security or performance — DevSecOps owns those.",
                "system_prompt": _SDET_SYSTEM_TEMPLATE,
                "tools": [
                    "read_file", "search_code", "search_codebase", "list_files",
                    "view_signatures", "run_command",
                    "write_file", "edit_file", "revert_last_change",
                    "web_fetch",
                    "browser_act",
                ],
                "model": "deepseek:v4-flash",
                "reasoning_effort": "high",
            },
            {
                "name": "FreeAgent",
                "description": "FreeAgent â€” general-purpose free model agent (OpenRouter, Groq, Google AI Studio, Mistral, NVIDIA NIM, Cloudflare, Z.ai, OpenCode Zen, Kilo, Cline). Handles text chat, vision analysis, research, document creation, web browsing. Zero cost.",
                "system_prompt": _FREE_AGENT_SYSTEM_TEMPLATE,
                "tools": [
                    "web_fetch", "web_search",
                    "browser_navigate", "browser_extract", "browser_screenshot",
                    "browser_click", "browser_fill", "browser_close",
                    "create_docx", "create_xlsx", "read_pdf",
                ],
                "model": "free/free-agent",
            },
        ]
        for s in subs:
            # CF1: deterministic tool order — `set()` iteration is hash-randomized
            # across processes, which would make each subagent SystemMessage bytes
            # differ per restart/subprocess and silently invalidate the DeepSeek
            # prefix cache. dict.fromkeys preserves first-occurrence order.
            # D2: terminal-node subagents (Dev/BSA/DevSecOps/SDET/FreeAgent) get NO
            # delegation tools — every prompt declares them terminal nodes and the
            # orchestrator is the only delegator. general-purpose keeps them below.
            s["tools"] = list(dict.fromkeys(s.get("tools", [])))
            _SUBAGENTS_REGISTRY[s["name"]] = s

        # Register explicit aliases for backward compatibility and tests
        aliases = {
            "BSA-Requirements": "BSA",
            "BSA-Schema": "BSA",
            "BSA-API": "BSA",
            "BA-GapAnalyzer": "BSA",
            "BA-Gherkin": "BSA",
            "SA-Database": "BSA",
            "SA-API": "BSA",
            "DevSecOps-Pipeline-Docker": "DevSecOps",
            "DevSecOps-Issues": "DevSecOps",
            "DevOps-Pipeline-Docker": "DevSecOps",
            "DevOps-Issues": "DevSecOps",
            "CodeCritic": "SDET",
            "Vision": "FreeAgent",
            "BA": "BSA",
            "SA": "BSA",
            "DevOps": "DevSecOps",
            "Refinement": "DevSecOps",
            "Analytics": "BSA",
            "Critic": "SDET",
            "Designer": "BSA",
            "Security": "DevSecOps",
        }
        for alias, base_name in aliases.items():
            if base_name in _SUBAGENTS_REGISTRY:
                alias_spec = dict(_SUBAGENTS_REGISTRY[base_name])
                alias_spec["name"] = alias
                _SUBAGENTS_REGISTRY[alias] = alias_spec
    except Exception as e:
        print(f"Error loading modularized subagents: {e}")

    # General-purpose subagent â€” has ALL tools including delegation
    _SUBAGENTS_REGISTRY["general-purpose"] = {
        "name": "general-purpose",
        "description": "General-purpose subagent for any complex multi-step task. Has all tools including the ability to spawn sub-subagents. Use for large tasks that need decomposition.",
        "system_prompt": "You are a general-purpose AI agent. You have full tool access including the ability to spawn subagents. For large tasks, decompose into parallel subtasks using start_async_task. Check results with check_async_task. Synthesize and return a complete result.",
        "tools": ["read_file", "write_file", "edit_file", "run_command", "search_code",
                   "list_files", "view_signatures", "task", "start_async_task",
                   "check_async_task", "list_async_tasks", "compact_conversation"],
        "model": "deepseek:v4-pro",
        "response_format": None,
    }

_HANDOFF_MAX_CHARS = 1000  # 1000 chars — compact structured report; oversized text offloads to VFS


def _classify_error(exc: str) -> str:
    """Map an exception name to an error category label."""
    if not exc:
        return ""
    e = exc.lower()
    if e in ("keyerror", "indexerror", "attributeerror", "typeerror", "valueerror", "zerodivisionerror", "overflowerror", "nan"):
        return "CODE_BUG (Runtime Logic Error)"
    if e in ("syntaxerror", "indentationerror", "modulenotfounderror", "importerror", "nameerror", "referenceerror"):
        return "CODE_BUG (Syntax/Import Error)"
    if e in ("assertionerror",) or "assert" in e:
        return "TEST_FAILURE (Assertion)"
    if e in ("filenotfounderror", "permissionerror", "connectionerror", "connectionrefusederror", "timeouterror", "econnrefused"):
        return "ENV_ISSUE (Missing resource/connectivity)"
    if e.endswith("error") or e.endswith("exception"):
        return "RUNTIME_ERROR"
    return "UNKNOWN"


def _compact_handoff(content, name: str = "subagent", vfs_path: str = "") -> str:
    """Compress a subagent's final message into the AGENT HANDOFF REPORT format.

    Subagents report back to another LLM (the orchestrator), not a human. The report
    only needs: status, files, error classification, failure locus, and a minimal
    error trail. No prose, no full tracebacks, no code dumps (the orchestrator
    re-delegates with the gap; it never fixes from a report).
    Hard cap: 1000 chars. Full raw text offloads to VFS with a pointer line.

    Strategy: (1) pass through the model's own report if it followed the format;
    (2) lenient fallback â€” build the report from the raw output via heuristics.
    """
    import re as _re

    content = str(content or "").strip()
    if not content:
        content = "Status: DONE - no output produced."

    # 1) Model followed the report format -> normalize + cap
    if "AGENT HANDOFF REPORT" in content:
        idx = content.find("### AGENT HANDOFF REPORT")
        report = content[idx:].strip()
        # cut trailing prose after the final code fence
        fence = report.rfind("```")
        if fence != -1 and len(report) - fence < _HANDOFF_MAX_CHARS:
            report = report[:fence + 3]
        return report[:_HANDOFF_MAX_CHARS]

    # 2) Heuristic build from raw output
    low = content.lower()
    if any(k in low for k in ("unable", "stuck", "error", "failed", "exception", "traceback", "exit code 1", "did not pass", "tests failed")):
        status = "FAILED"
    elif any(k in low for k in ("blocked", "cannot", "waiting", "needs")):
        status = "BLOCKED"
    else:
        status = "DONE"

    lines = [l.strip() for l in content.splitlines() if l.strip()]
    tail = "\n".join(lines[-5:])

    # Error classification
    m_exc = _re.search(r"(\w+(?:Error|Exception|Failure|Warning|Timeout))", content)
    exc = m_exc.group(1) if m_exc else ""
    m_cat = _re.search(r"Error Category[:\s]*`?([^`\n]+)", content, _re.IGNORECASE)
    category = m_cat.group(1).strip() if m_cat else (_classify_error(exc) if exc else "")
    m_loc = _re.search(r"([\w./\\-]+\.(?:py|js|ts|php|go|rs|java|rb))[::](\d+)", content)
    loc = f"{m_loc.group(1)} (Line {m_loc.group(2)})" if m_loc else ""
    m_fn = _re.search(r"\bin\s+(\w+)\s*\(", content) or _re.search(r"\bdef\s+(\w+)\s*\(", content)
    fn = (m_fn.group(1) + "()") if m_fn else ""
    m_cmd = _re.search(r"(?:^|\n)[^\n]*(pytest|npm (?:test|run|install)|node |python\s+|go test|make |npx )", content)
    cmd = m_cmd.group(0).strip()[:120] if m_cmd else ""

    # Files touched
    files = []
    for l in lines:
        cf = l.lstrip("-*[] ").strip().rstrip(",")
        if any(c in cf for c in ("/", "\\", ".")) and len(cf) < 200 and not cf.startswith(("PASS", "FAIL", "Traceback", "File ")):
            files.append(cf)
        if len(files) >= 10:
            break

    # Trigger: prefer a line with expected/should/when, else a short leading line
    trigger = ""
    for l in lines:
        if _re.search(r"\b(trigger|expected|should|when|because)\b", l, _re.IGNORECASE) and len(l) < 240:
            trigger = l
            break
    if not trigger:
        first = lines[0] if lines else ""
        if len(first) < 240:
            trigger = first

    vfs_line = f"- **VFS Log Path:** `{vfs_path}` (Full log offloaded)\n" if vfs_path else ""
    if status == "DONE":
        out = ["### AGENT HANDOFF REPORT [DONE]",
               f"- **Agent:** {name} | **Status:** DONE",
               vfs_line.rstrip("\n"),
               f"- **Files:** {', '.join('`%s`' % f for f in files[:10]) if files else '(none)'}",
               "",
               "#### 1. OUTCOME",
               f"- {trigger or 'Completed.'}"]
    else:
        out = [f"### AGENT HANDOFF REPORT [{status}]",
               f"- **Agent:** {name} | **Status:** {status}",
               vfs_line.rstrip("\n"),
               "",
               "#### 1. ERROR CLASSIFICATION",
               f"- **Error Category:** `{category}`" if category else "- **Error Category:** UNKNOWN",
               f"- **Primary Exception:** `{exc}`" + (f" in `{loc}`" if loc else "") if exc else "",
               f"- **Failing Command:** `{cmd}`" if cmd else "",
               "",
               "#### 2. FAILURE LOCUS & CONTEXT",
               f"- **File:** `{loc}`" if loc else "- **File:** (not identified)",
               f"- **Function:** `{fn}`" if fn else "",
               f"- **Trigger:** {trigger}" if trigger else "",
               "",
               "#### 3. MINIMAL ERROR TRAIL (Tail 5 Lines)",
               "```text",
               tail,
               "```",
               "",
               f"- **Files:** {', '.join('`%s`' % f for f in files[:10]) if files else '(none)'}",
               f"- **Next:** re-delegate to `Dev`" if status == "FAILED" else ""]
    report = "\n".join(x for x in out if x)
    return report[:_HANDOFF_MAX_CHARS]


def _handoff(content, name: str = "subagent") -> str:
    """Full handoff pipeline: offload raw output to VFS when it exceeds the cap,
    then produce the compact AGENT HANDOFF REPORT with a pointer to the full log."""
    import uuid
    content = str(content or "").strip()
    vfs_path = ""
    if len(content) > _HANDOFF_MAX_CHARS:
        try:
            file_id = uuid.uuid4().hex[:8]
            clean = "".join(c if c.isalnum() else "_" for c in name)[:40] or "subagent"
            vfs_path = f"/scratch/handoff_log_{clean}_{file_id}.txt"
            write_file(vfs_path, content)
        except Exception:
            vfs_path = ""
    return _compact_handoff(content, name, vfs_path)


def _log_taxonomy(reason: str, detail: str) -> None:
    """B8: log the error-taxonomy classification + handler policy for requery events."""
    try:
        from agent_errors import classify_tool_error, handler_policy
        from state_sync import safe_append_live_log
        err_cls = classify_tool_error(f"{reason}: {detail}")
        safe_append_live_log(f"[SUBAGENT] {reason} ({detail}) — taxonomy: {err_cls.__name__}, policy: {handler_policy(err_cls)}")
    except Exception:
        pass


def _private_requery_subagent(role, messages, failed_content, subagent_model, subagent_effort, schema, max_sub_iters) -> tuple:
    """
    A2: private re-query for unparsable tool calls in the generic subagent loop.

    Re-queries with the error template WITHOUT appending the failed turn to
    history (SWE-agent requery pattern). Returns (content, tool_calls) on
    recovery, None when the model is still unparsable (caller falls back to a
    history nudge).
    """
    error_template = (
        "Your previous message looked like it was trying to call a tool, but "
        "the tool call did not parse. Output ONLY a valid tool call in this "
        "exact format:\n"
        '```tool\n{"tool": "tool_name", "args": {"param": "value"}}\n```\n'
        "Ensure the JSON is valid. If you are truly done, reply in plain "
        "text with no ```tool block."
    )
    try:
        from developer_agent import _parse_tool_call
        import re as _re2
        res = invoke_messages_with_fallback(
            role=role,
            messages=list(messages) + [
                AIMessage(content=failed_content),
                HumanMessage(content=error_template),
            ],
            schema=schema,
            temp=0.2,
            model=subagent_model,
            reasoning_effort=subagent_effort,
        )
        new_content = str(res)
        if schema and (hasattr(res, "model_dump_json") or hasattr(res, "json")):
            return None  # structured-output path: no tool loop
        tool_calls = []
        for match in _re2.finditer(r'```tool\s*\n(.*?)\n```', new_content, _re2.DOTALL):
            tc = _parse_tool_call(match.group(0))
            if tc:
                tool_calls.append(tc)
        if not tool_calls:
            tc = _parse_tool_call(new_content)
            if tc:
                tool_calls.append(tc)
        if not tool_calls:
            return None
        return new_content, tool_calls
    except Exception:
        return None


def task(name: str, task: str, reasoning_effort: Optional[str] = None) -> str:
    """Delegate a specialized subtask to a subagent with full tool access.

    Spawns a subagent with its own system prompt, tools, and model. The subagent
    runs a full agent loop (LLM â†’ tools â†’ observe â†’ repeat) in ISOLATED context.
    The parent agent never sees the subagent's intermediate work â€” only the result.

    Args:
        name: Subagent type (e.g. BA, SA, DevOps, Security, Analytics, Dev).
        task: Detailed instruction describing what the subagent should accomplish.
        reasoning_effort: Optional thinking budget override for the subagent run
            (\"max\" / \"high\" / \"medium\" / \"low\" / \"disabled\"). Defaults to
            the registry spec (SDET/DevSecOps = max). Use \"disabled\" for purely
            mechanical subagents, \"high\"/\"max\" for deep root-cause work.
    """
    depth = _TASK_DEPTH.get()
    if depth >= _MAX_TASK_DEPTH:
        return f"Error: Maximum task depth ({_MAX_TASK_DEPTH}) reached. Cannot spawn subagent '{name}' at depth {depth}."

    if not _SUBAGENTS_REGISTRY:
        _load_subagents()

    if name not in _SUBAGENTS_REGISTRY:
        allowed = ", ".join(_SUBAGENTS_REGISTRY.keys())
        return f"Error: Unknown subagent '{name}'. Available subagents: {allowed}"

    spec = _SUBAGENTS_REGISTRY[name]
    sys_prompt = spec.get("system_prompt", "You are a specialized subagent.")
    tool_names = spec.get("tools", [])
    subagent_model = spec.get("model")
    # Only enforce structured output schema if subagent has NO tools.
    # When tools are available, the agent loop handles output naturally.
    schema = spec.get("response_format") if not tool_names else None

    # Native function calling for the generic loop (SDET / DevSecOps / others):
    # the native schemas are THE single source of tool documentation. Filtered
    # to the registry's allow-list; text-format tool calls remain as fallback.
    native_tools = None
    if schema is None and tool_names:
        try:
            from tool_schemas import get_native_tools
            native_tools = [t for t in get_native_tools()
                            if t.get("function", {}).get("name") in tool_names]
            if not native_tools:
                native_tools = None
        except Exception:
            native_tools = None

    # Build tool definitions for this subagent
    subagent_tools_str = ""
    if tool_names:
        # Compact one-line tool defs (mirrors developer_agent.py format)
        _COMPACT_DEFS = {
            "read_file": "read_file(file_path, offset?, limit?) â€” Read file.",
            "write_file": "write_file(file_path, content) â€” Create or overwrite a file.",
            "edit_file": "edit_file(file_path, old_string?, new_string?, diff?) â€” Edit file.",
            "revert_last_change": "revert_last_change(file_path) â€” Restore the most recent snapshot (undo the last write/edit to this file).",
            "run_command": "run_command(command, timeout?, background?) — Run a SINGLE plain shell command. NO PowerShell syntax: no ';' chaining, no '|' pipes, no Select-Object/Get-Content/Remove-Item/Test-Path — plain cmd-compatible commands only.",
            "search_code": "search_code(pattern, path?, glob?) â€” Regex search across files.",
            "list_files": "list_files(path?, pattern?, recursive?) â€” List directory contents.",
            "view_signatures": "view_signatures(file_path) â€” Extract function/class signatures.",
            "search_codebase": "search_codebase(query, top_k?) â€” Semantic code search.",
            "web_fetch": "web_fetch(url, max_chars?) â€” Fetch URL via HTTP GET (fast, no browser).",
            "browser_navigate": "browser_navigate(url, wait_ms?) â€” Open URL in headless Chromium, returns snapshot with @ref selectors.",
            "browser_act": "browser_act(action, selector?, text?, key?, what?, path?) â€” click / fill / type / press / extract / screenshot / close.",
            "browser_extract": "browser_extract(what?, selector?) â€” Extract text/html/value from current browser page.",
            "browser_screenshot": "browser_screenshot(path?) â€” Screenshot of current browser page.",
            "browser_click": "browser_click(selector) â€” Click an element using CSS selector or snapshot @ref tag.",
            "browser_fill": "browser_fill(selector, text) â€” Clear and fill an input element.",
            "browser_type": "browser_type(selector, text) â€” Type text into an input element.",
            "browser_press": "browser_press(key) â€” Press a keyboard key (e.g. 'Enter').",
            "browser_close": "browser_close() â€” Close browser session and free resources.",
            "create_docx": "create_docx(file_path, title?, content?) â€” Create a Word document (.docx).",
            "create_xlsx": "create_xlsx(file_path, headers, rows) â€” Create an Excel spreadsheet (.xlsx).",
            "read_pdf": "read_pdf(file_path, max_pages?, page_numbers?) â€” Read text from a PDF file.",
            "web_search": "web_search(query) â€” Search the web for current information.",
            "task": "task(name, task, reasoning_effort?) â€” Delegate a subtask to a subagent (delegators only).",
            "start_async_task": "start_async_task(name, task) â€” Fire-and-forget parallel subagent; returns a task_id.",
            "check_async_task": "check_async_task(task_id, wait_seconds?) â€” Poll an async task; optionally wait.",
            "list_async_tasks": "list_async_tasks() â€” List async tasks and statuses.",
            "compact_conversation": "compact_conversation() â€” Request context compaction (runs at turn end).",
        }
        # Every allow-listed tool MUST surface in the prompt — a tool that is granted
        # but undocumented is invisible to the model. No filtering here.
        tool_lines = [_COMPACT_DEFS.get(t, f"{t}() â€” Execute {t}") for t in tool_names]
        subagent_tools_str = "## Available Tools\n" + "\n".join(f"- {line}" for line in tool_lines)

    full_sys_prompt = sys_prompt
    if tool_names and native_tools is None:
        # Text-mode fallback (no native schemas): document the tools in-prompt.
        full_sys_prompt += (
            "\n\n## Tool Usage\n"
            "You have tools to read, write, and execute. USE THEM to complete tasks.\n"
            "Call tools using this format:\n"
            "```tool\n"
            '{"tool": "tool_name", "args": {"param": "value"}}\n'
            "```\n"
            "Chain multiple tools in one response when you know what you need.\n"
            "Do NOT just describe what you would do â€” actually call the tools.\n"
            "Stop calling tools when the task is complete."
        )
        full_sys_prompt += "\n\n" + subagent_tools_str

    # Cached repository map for code-touching subagents (SDET / DevSecOps) —
    # appended LAST in the system message. Byte-identical per project, so it
    # cache-hits on every session after the first; append-only growth only
    # ever touches the map's bottom (prefix stays cached). Read-only loader:
    # generation/persistence happens in refresh_code_map. The project identity
    # card follows it at the very tail (rare [updated:] appends stay clean).
    if ("sdet" in str(name).lower() or "devsecops" in str(name).lower()
            or "devops" in str(name).lower()):
        try:
            from memory_manager import load_cached_code_map, load_cached_project_identity
            _map_project = shared_state.get("project_path", _HARNESS_ROOT)
            _cached_map = load_cached_code_map(_map_project)
            if _cached_map:
                full_sys_prompt += "\n\n## Repository Map (cached, append-only)\n" + _cached_map
            _project_card = load_cached_project_identity(_map_project)
            if _project_card:
                full_sys_prompt += "\n\n## Project Overview\n" + _project_card
        except Exception as e:
            print(f"[tools] Cached context load error for {name}: {e}")

    # Permissions
    raw_perms = spec.get("permissions", [])
    perms = []
    for p in raw_perms:
        if isinstance(p, dict):
            perms.append(FilesystemPermission(
                operations=p.get("operations", []),
                paths=p.get("paths", []),
                mode=p.get("mode", "allow")
            ))
        elif isinstance(p, FilesystemPermission):
            perms.append(p)

    depth_token = _TASK_DEPTH.set(depth + 1)
    token = active_permissions.set(perms)
    try:
        chat_id = shared_state.get("chat_id", "default_chat")
        if name == "Dev":
            from developer_agent import developer_node
            from state_sync import safe_update_state
            
            project_path = shared_state.get("project_path", _HARNESS_ROOT)
            
            dev_state = {
                "client_request": task,
                "tech_spec": shared_state.get("outputs", {}).get("tech_spec", ""),
                "requirements": shared_state.get("outputs", {}).get("requirements", ""),
                "project_path": project_path,
                "chat_id": chat_id,
                "error_count": 0,
                "test_report": "",
            }
            
            # Orchestrator dial: an explicit reasoning_effort override sets Dev's
            # planning depth for THIS delegation (default stays BSA_REASONING_ITER1).
            _prev_iter1 = os.environ.get("BSA_REASONING_ITER1")
            if reasoning_effort:
                os.environ["BSA_REASONING_ITER1"] = reasoning_effort
            try:
                res_dict = developer_node(dev_state)
            finally:
                if _prev_iter1 is not None:
                    os.environ["BSA_REASONING_ITER1"] = _prev_iter1
                else:
                    os.environ.pop("BSA_REASONING_ITER1", None)
            
            # Sync outputs back to shared_state
            if "code" in res_dict and res_dict["code"]:
                safe_update_state({
                    "outputs": {
                        **shared_state.get("outputs", {}),
                        "code": res_dict["code"],
                        "test_report": res_dict.get("test_report", ""),
                    }
                })
            
            return _handoff(res_dict.get("agent_report") or res_dict.get("code") or "Developer completed execution.", "Dev")

        # Free-only BSA spec agent â€” harness-enforced incremental SPEC.md generation with
        # zero cost (no paid DeepSeek fallback). Mirrors the BSA alias names registered in
        # _load_subagents (BA, SA, BSA-*, Analytics, Designer) so they build specs this way too.
        if name == "BSA" or name in {"BA", "SA", "BSA-Requirements", "BSA-Schema",
                                      "BSA-API", "BA-GapAnalyzer", "BA-Gherkin",
                                      "SA-Database", "SA-API", "Analytics", "Designer"}:
            from bsa_spec_agent import run_bsa_spec
            project_path = shared_state.get("project_path", _HARNESS_ROOT)
            return _handoff(run_bsa_spec(task, project_path, chat_id), name)

        # FreeAgent routes through the free-provider implementation (OpenRouter/Groq/Gemini/
        # Mistral/NVIDIA/Cloudflare), NOT the DeepSeek client. Previously it fell through to the
        # DeepSeek generic loop with model "free/free-agent" â†’ 400 error from the DeepSeek API.
        if name == "FreeAgent":
            from free_agent import build_general_profile_prompt, free_agent_node
            resolved_task = task
            if shared_state.get("is_general", False):
                try:
                    prefix = build_general_profile_prompt()
                    if prefix:
                        resolved_task = prefix + task
                except Exception:
                    pass
            return _handoff(free_agent_node(resolved_task), name)

        # All other subagents (Dev handled above; SDET, DevSecOps, BSA, FreeAgent, etc.)
        # fall through to the generic DeepSeek agent loop with the raw task.
        resolved_task = task

        messages = load_subagent_history(chat_id, name)
        if not messages:
            messages.append(SystemMessage(content=full_sys_prompt))
        messages.append(HumanMessage(content=resolved_task))

        from llm import invoke_messages_with_fallback

        # Orchestrator dial overrides the registry default for this delegation
        # (registry keeps e.g. max for SDET/DevSecOps when no override is given).
        subagent_effort = reasoning_effort or spec.get("reasoning_effort")

        # ── Subagent loop: LLM → tools → observe → repeat ──
        # Budget knob: bounds a single subagent session (SDET-FIX included).
        # Default 25: the fixer persists until the task is fulfilled, bounded
        # against runaway cost.
        max_sub_iters = int(os.environ.get("BSA_SUBAGENT_MAX_ITERS", "25"))
        content = ""
        # D4: DevSecOps host validation — track files written so the prompt's
        # "host validates your files" rule is backed by an actual check.
        written_files: dict = {}
        _is_devsecops = ("devsecops" in str(name).lower() or "devops" in str(name).lower())
        # Adaptive reasoning ladder (same pattern as Dev/Orchestrator):
        # iter-1 (diagnosis) uses the spec effort (max for SDET); routine turns
        # run disabled; a trouble signal (tool error / failure markers) flips
        # the NEXT turn back to the spec effort for diagnosis.
        _sub_trouble = False
        _routine_effort = os.environ.get("BSA_REASONING_SUBAGENT_ROUTINE", "disabled")
        # Verification-before-DONE guard (fixer subagents only): SDET must not
        # exit with edits made after its last verification run.
        _is_sdet_fixer = "sdet" in str(name).lower()
        _last_edit_iter = -1
        _last_run_iter = -1
        _last_tool_name = ""
        _vfd_nudges = 0
        try:
            for sub_iter in range(max_sub_iters):
                # Call LLM
                if sub_iter == 0 or _sub_trouble:
                    _effort = subagent_effort
                    _sub_trouble = False
                else:
                    _effort = _routine_effort
                res = invoke_messages_with_fallback(
                    role=name,
                    messages=list(messages),
                    schema=schema,
                    temp=0.2,
                    model=subagent_model,
                    reasoning_effort=_effort,
                    tools=native_tools,
                    where=f"{name} iter {sub_iter + 1}/{max_sub_iters}: {_last_tool_name or 'start'}",
                )

                # Extract response text
                additional_kwargs = {}
                reasoning = getattr(res, "reasoning_content", None)
                if reasoning:
                    additional_kwargs["reasoning_content"] = reasoning

                if schema and hasattr(res, "model_dump_json"):
                    content = res.model_dump_json()
                    messages.append(AIMessage(content=content, additional_kwargs=additional_kwargs))
                    break  # Structured output â†’ done
                elif schema and hasattr(res, "json"):
                    content = res.json()
                    messages.append(AIMessage(content=content, additional_kwargs=additional_kwargs))
                    break
                else:
                    # Native mode returns (text, native_tool_calls, reasoning)
                    native_tool_calls = []
                    if isinstance(res, tuple):
                        if len(res) == 3:
                            response_text, native_tool_calls, reasoning = res
                        else:
                            response_text, native_tool_calls = res
                            reasoning = None
                        if reasoning:
                            additional_kwargs["reasoning_content"] = reasoning
                    else:
                        response_text = str(res)
                    content = response_text

                # Parse tool calls: native tool_calls first, text blocks as fallback
                from developer_agent import _parse_tool_call
                tool_calls = []
                if native_tool_calls:
                    for tc in native_tool_calls:
                        tool_calls.append({
                            "tool": tc.get("tool") or tc.get("name", ""),
                            "args": tc.get("args") or {},
                            "id": tc.get("id", ""),
                        })
                else:
                    # Multi-tool parsing: find all ```tool``` blocks in the response
                    import re as _re2
                    for match in _re2.finditer(r'```tool\s*\n(.*?)\n```', content, _re2.DOTALL):
                        tc = _parse_tool_call(match.group(0))
                        if tc:
                            tool_calls.append(tc)
                    # Also try parsing the entire response as a single tool call
                    if not tool_calls:
                        tc = _parse_tool_call(content)
                        if tc:
                            tool_calls.append(tc)

                if not tool_calls:
                    # If the response looks like it tried a tool call but didn't parse,
                    # re-query the model privately (A2: the failed turn is NOT appended
                    # to history — SWE-agent requery pattern). Falls back to a history
                    # nudge only when recovery fails.
                    from developer_agent import looks_like_tool_call
                    if looks_like_tool_call(content) and sub_iter < max_sub_iters - 1:
                        recovered = _private_requery_subagent(
                            role=name, messages=messages, failed_content=content,
                            subagent_model=subagent_model, subagent_effort=subagent_effort,
                            schema=schema, max_sub_iters=max_sub_iters,
                        )
                        if recovered is not None:
                            content, tool_calls = recovered
                            _log_taxonomy("format", "recovered via private re-query")
                            # Record the RECOVERED response (clean call) in history
                            # (the "Record the AI response" block below appends it)
                        else:
                            _log_taxonomy("format", "no recovery after private re-query")
                            messages.append(AIMessage(content=content, additional_kwargs=additional_kwargs))
                            messages.append(HumanMessage(
                                content=(
                                    "Your previous message looked like it was trying to call a tool, but "
                                    "the tool call did not parse. Output ONLY a valid tool call in this "
                                    "exact format:\n"
                                    '```tool\n{"tool": "tool_name", "args": {"param": "value"}}\n```\n'
                                    "Ensure the JSON is valid. If you are truly done, reply in plain "
                                    "text with no ```tool block."
                                )
                            ))
                            continue
                    else:
                        # No tools → subagent is done
                        messages.append(AIMessage(content=content, additional_kwargs=additional_kwargs))
                        # Verification-before-DONE guard (SDET only): never let the
                        # fixer claim DONE with edits made after its last test run,
                        # and never let it claim DONE without attempting ANY fix.
                        _no_work = (_last_edit_iter < 0 and _last_run_iter < 0)
                        if (_is_sdet_fixer and _vfd_nudges < 1 and sub_iter < max_sub_iters - 2
                                and (_last_edit_iter > _last_run_iter or _no_work)):
                            _vfd_nudges += 1
                            if _no_work:
                                _nudge_text = (
                                    "[VERIFY BEFORE DONE] You have not changed ANY file and ran "
                                    "no verification. The failure requires a FIX — read the "
                                    "CONTRACT, write the fix (edit_file/write_file), then run the "
                                    "failing command / the project's test until it PASSES. "
                                    "Never report DONE without attempting a fix."
                                )
                            else:
                                _nudge_text = (
                                    "[VERIFY BEFORE DONE] You are about to finish, but you edited "
                                    "files after your last verification run — your fix is "
                                    "UNVERIFIED. RUN the failing command / the project's test now "
                                    "(or your repro script), read the ACTUAL output, and only then "
                                    "report DONE. Never claim a fix you did not run."
                                )
                            messages.append(HumanMessage(content=_nudge_text))
                            _log_taxonomy("verify", "fixer tried to exit unverified/without work — nudged")
                            continue
                        break

                # Record the AI response (native: carry tool_calls for the API round-trip)
                if native_tool_calls and tool_calls:
                    messages.append(AIMessage(
                        content=content,
                        tool_calls=[{"name": tc["tool"], "args": tc.get("args") or {},
                                     "id": tc.get("id", "")} for tc in tool_calls],
                        additional_kwargs=additional_kwargs,
                    ))
                else:
                    messages.append(AIMessage(content=content, additional_kwargs=additional_kwargs))

                # Execute tools and collect results
                tool_results = []
                for tc in tool_calls:
                    tool_name = tc.get("tool", "")
                    args = tc.get("args", {})
                    if tool_name not in tool_names and tool_names:
                        tool_results.append(f"[SKIPPED] Tool '{tool_name}' not in subagent permissions.")
                        continue
                    try:
                        result = execute_tool(tool_name, args)
                        result = auto_offload_result(str(result), tool_name, max_chars=_tool_cap("subagent"))
                        tool_results.append(f"[{tool_name}] {result}")
                        # D4: capture write_file content for DevSecOps host validation
                        if _is_devsecops and tool_name == "write_file" and isinstance(args.get("content"), str):
                            written_files[args.get("file_path") or "unknown"] = args["content"]
                        # Verification-before-DONE bookkeeping
                        _last_tool_name = tool_name
                        if _is_sdet_fixer:
                            if tool_name in ("write_file", "edit_file", "revert_last_change"):
                                _last_edit_iter = sub_iter
                            elif tool_name == "run_command":
                                _last_run_iter = sub_iter
                    except Exception as e:
                        tool_results.append(f"[{tool_name}] ERROR: {str(e)[:200]}")

                # Feed results back (native round-trip uses ToolMessage with ids)
                feedback = "\n".join(tool_results) if tool_results else "No tool results."
                # Trouble signal → the NEXT turn diagnoses with the spec effort
                _j = feedback.lower()
                if any(k in _j for k in ("[error]", "[skipped]", "failed", "exception",
                                         "traceback", "timeout", "not found", "cannot find module")):
                    _sub_trouble = True
                if native_tool_calls and tool_calls and all(tc.get("id") for tc in tool_calls):
                    for tc, tr in zip(tool_calls, tool_results):
                        messages.append(ToolMessage(content=tr, tool_call_id=tc["id"]))
                else:
                    messages.append(HumanMessage(content=f"Tool results:\n{feedback}"))

            # No budget checks here; subagents are managed by LoopGuard and structural loops.

            # D4: run host validation on DevSecOps-written files (Rule 4 of its prompt)
            if _is_devsecops and written_files:
                try:
                    from devops_verifier import verify_devops_files
                    v_errors = verify_devops_files(written_files)
                    if v_errors:
                        content = f"{content}\n\n[HOST VALIDATION]\n" + "\n".join(v_errors[:15])
                except Exception:
                    pass

        except Exception as e:
            save_subagent_history(chat_id, name, messages)
            raise e

        # Save and clear history
        save_subagent_history(chat_id, name, messages)
        clear_subagent_history(chat_id, name)

    finally:
        active_permissions.reset(token)
        _TASK_DEPTH.reset(depth_token)

    # Full output beyond the handoff cap is kept on disk (VFS) with a pointer in the
    # report; the handoff itself stays compact so the orchestrator's context never
    # eats a multi-KB dump.
    return _handoff(content, name)


# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
# Async Task Tools â€” fire-and-forget subagent spawning
# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

def _run_subagent_in_thread(task_id: str, name: str, task_desc: str, depth: int) -> None:
    """Run a subagent in a background thread. Sets result/error on _ASYNC_TASKS."""
    try:
        _TASK_DEPTH.set(depth)
        result = task(name, task_desc)
        with _ASYNC_LOCK:
            if task_id in _ASYNC_TASKS:
                _ASYNC_TASKS[task_id]["status"] = "completed"
                _ASYNC_TASKS[task_id]["result"] = result
    except Exception as e:
        with _ASYNC_LOCK:
            if task_id in _ASYNC_TASKS:
                _ASYNC_TASKS[task_id]["status"] = "failed"
                _ASYNC_TASKS[task_id]["error"] = str(e)[:500]


def start_async_task(name: str, task: str) -> str:
    """Fire-and-forget subagent. Returns task_id immediately. Subagent runs in background.

    Use this to spawn MULTIPLE subagents in parallel. Poll with check_async_task().
    For immediate results, use the regular task() tool instead.

    Args:
        name: Subagent type (e.g. 'DevOps', 'BA')
        task: Detailed task description for the subagent
    """
    import uuid
    depth = _TASK_DEPTH.get()
    if depth >= _MAX_TASK_DEPTH:
        return f"Error: Maximum task depth ({_MAX_TASK_DEPTH}) reached. Cannot spawn subagent at depth {depth}."

    if not _SUBAGENTS_REGISTRY:
        _load_subagents()
    if name not in _SUBAGENTS_REGISTRY:
        allowed = ", ".join(_SUBAGENTS_REGISTRY.keys())[:200]
        return f"Error: Unknown subagent '{name}'. Available: {allowed}"

    task_id = f"{name}-{uuid.uuid4().hex[:6]}"
    with _ASYNC_LOCK:
        _ASYNC_TASKS[task_id] = {
            "status": "running",
            "name": name,
            "task": task,
            "depth": depth,
            "started_at": __import__('time').time(),
        }

    # Set depth for subagent's thread context
    child_depth = depth + 1
    _ASYNC_EXECUTOR.submit(_run_subagent_in_thread, task_id, name, task, child_depth)

    return f"Task started: {task_id}\nSubagent: {name}\nStatus: RUNNING"


def check_async_task(task_id: str, wait_seconds: int = 0) -> str:
    """Check the status of an async task. Returns result if completed.

    Args:
        task_id: The task ID returned by start_async_task()
        wait_seconds: If task is still running, wait up to this many seconds
                      before returning the status. Lets long tasks finish naturally
                      without polling. Max 300 seconds (5 minutes).
    """
    import time as time_mod
    wait_seconds = max(0, min(wait_seconds, 300))  # clamp 0-300
    with _ASYNC_LOCK:
        if task_id not in _ASYNC_TASKS:
            return f"Error: Unknown task '{task_id}'. Use list_async_tasks() to see active tasks."
        t = _ASYNC_TASKS[task_id]
        if t["status"] == "running":
            # If wait_seconds is set, sleep in small intervals and check again
            if wait_seconds > 0:
                _ASYNC_LOCK.release()
                try:
                    check_interval = 2  # check every 2 seconds
                    waited = 0
                    while waited < wait_seconds:
                        time_mod.sleep(min(check_interval, wait_seconds - waited))
                        waited += min(check_interval, wait_seconds - waited)
                        with _ASYNC_LOCK:
                            if task_id not in _ASYNC_TASKS:
                                return f"Error: Task '{task_id}' no longer exists."
                            t2 = _ASYNC_TASKS[task_id]
                            if t2["status"] != "running":
                                if t2["status"] == "completed":
                                    result = t2.get("result", "")
                                    return f"STATUS: DONE\n{result}"
                                else:
                                    return f"STATUS: FAILED\n{t2.get('error', 'Unknown error')}"
                finally:
                    _ASYNC_LOCK.acquire()
                # After waiting, re-read current state
                t = _ASYNC_TASKS.get(task_id)
                if t is None:
                    return f"Error: Task '{task_id}' no longer exists."
                if t["status"] == "running":
                    elapsed = time_mod.time() - t.get("started_at", 0)
                    return f"STATUS: RUNNING (waited {wait_seconds}s, elapsed: {elapsed:.0f}s)\nTask: {t['name']}: {t['task'][:150]}"
                elif t["status"] == "completed":
                    result = t.get("result", "")
                    return f"STATUS: DONE\n{result}"
                else:
                    return f"STATUS: FAILED\n{t.get('error', 'Unknown error')}"
            else:
                elapsed = time_mod.time() - t.get("started_at", 0)
                return f"STATUS: RUNNING (elapsed: {elapsed:.0f}s)\nTask: {t['name']}: {t['task'][:150]}"
        elif t["status"] == "completed":
            result = t.get("result", "")
            return f"STATUS: DONE\n{result}"
        else:
            return f"STATUS: FAILED\n{t.get('error', 'Unknown error')}"


def list_async_tasks() -> str:
    """List all async tasks and their statuses."""
    import time
    with _ASYNC_LOCK:
        if not _ASYNC_TASKS:
            return "No async tasks."

        # Auto-cleanup completed tasks older than 10 minutes
        now = time.time()
        to_remove = []
        for tid, t in _ASYNC_TASKS.items():
            if t["status"] in ("completed", "failed"):
                age = now - t.get("started_at", now)
                if age > 600:  # 10 minutes
                    to_remove.append(tid)
        for tid in to_remove:
            del _ASYNC_TASKS[tid]

        if not _ASYNC_TASKS:
            return "No async tasks."

        lines = [f"{len(_ASYNC_TASKS)} async tasks:"]
        for tid, t in sorted(_ASYNC_TASKS.items()):
            status_icon = {"running": "[>]", "completed": "[OK]", "failed": "[XX]"}.get(t["status"], "[??]")
            lines.append(f"  {status_icon} [{t['status'].upper()}] {tid}: {t['name']} â€” {t['task'][:100]}")
        return "\n".join(lines)


def compact_conversation() -> str:
    """Request on-demand compaction of the conversation message history."""
    return "[OK] Conversation compaction has been requested and will be executed at the end of this turn."


# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
# â”€â”€ Pillar 98: Async Tool Pipelining â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

def pipeline_tools(steps: list[dict]) -> str:
    """
    Execute a sequence of tool calls in one LLM turn.
    Each step: {"tool": str, "args": dict}.
    If any step fails, aborts the pipeline and returns error context.

    Exposed as a tool to the agent â€” enables "write â†’ lint â†’ test" in a single call
    instead of 3 separate LLM roundtrips.
    """
    if not steps:
        return "Error: pipeline_tools requires at least one step."

    results: list[str] = []
    for i, step in enumerate(steps):
        tool_name = step.get("tool", "")
        tool_args = step.get("args", {})

        if not tool_name:
            results.append(f"[Step {i+1}] Error: No tool name specified.")
            break

        # Pre-validate
        valid, err = pre_validate_tool_args(tool_name, tool_args)
        if not valid:
            results.append(f"[Step {i+1}] Validation Error: {err}")
            results.append("PIPELINE ABORTED due to invalid arguments.")
            break

        # Execute
        res = execute_tool(tool_name, tool_args)
        results.append(f"[Step {i+1}] {tool_name}: {res[:500]}")

        # Abort on error
        if res.startswith("Error"):
            results.append(f"PIPELINE ABORTED at step {i+1}.")
            break

    return "\n".join(results)


# Tool Executor â€” maps tool names to functions
# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout / 1000)
        output = r.stdout.strip()
        if r.returncode != 0:
            err = r.stderr.strip()[:500]
            return f"(browser exited {r.returncode}: {err})" if err else f"(browser exited {r.returncode})"
        return output if output else "(empty output)"
    except subprocess.TimeoutExpired:
        return f"(browser command timed out after {timeout}ms)"
    except FileNotFoundError:
        return "(agent-browser not found. Run: npm i -g agent-browser && agent-browser install)"
    except Exception as e:
        return f"(browser error: {e})"


def web_fetch(url: str, max_chars: int = 10000) -> str:
    """Fetch a URL via plain HTTP GET and return text content. No browser overhead.

    Best for: documentation, API references, blog posts, static pages.
    NOT for: JavaScript-rendered SPAs or pages requiring login.

    Args:
        url: The full URL to fetch (https://...).
        max_chars: Maximum characters to return (default 10000).
    """
    if not url.startswith(("http://", "https://")):
        url = "https://" + url
    try:
        import ssl
        import urllib.request
        # Python installations on Windows do not always use the same CA store
        # as the browser. Prefer certifi's maintained CA bundle for research
        # requests so valid public HTTPS sites are not rejected as untrusted.
        try:
            import certifi
            ssl_context = ssl.create_default_context(cafile=certifi.where())
        except Exception:
            ssl_context = ssl.create_default_context()
        req = urllib.request.Request(url, headers={
            "User-Agent": "Mozilla/5.0 (compatible; ResearchBot/1.0)",
            "Accept": "text/html,text/plain,*/*",
        })
        with urllib.request.urlopen(req, timeout=15, context=ssl_context) as resp:
            raw = resp.read()
            # Try UTF-8 first, fallback to detected charset
            content_type = resp.headers.get("Content-Type", "")
            charset = "utf-8"
            if "charset=" in content_type:
                charset = content_type.split("charset=")[-1].split(";")[0].strip()
            try:
                text = raw.decode(charset)
            except (UnicodeDecodeError, LookupError):
                text = raw.decode("utf-8", errors="replace")

        # Strip HTML tags for a clean text version
        import re
        text = re.sub(r"<script[^>]*>.*?</script>", "", text, flags=re.DOTALL | re.IGNORECASE)
        text = re.sub(r"<style[^>]*>.*?</style>", "", text, flags=re.DOTALL | re.IGNORECASE)
        text = re.sub(r"<[^>]+>", " ", text)
        text = re.sub(r"\s+", " ", text).strip()

        if not text:
            return "(page returned no text content)"

        if len(text) > max_chars:
            text = text[:max_chars] + f"\n... (truncated, full length: {len(text)} chars)"

        return f"[WEB] {url}\n{text[:max_chars]}"
    except urllib.error.HTTPError as e:
        return f"(HTTP {e.code}: {e.reason} for {url})"
    except urllib.error.URLError as e:
        return f"(connection error: {e.reason})"
    except Exception as e:
        return f"(fetch error: {e})"


def browser_navigate(url: str, wait_ms: int = 5000) -> str:
    """Open a URL in a headless browser and return the page content (accessibility snapshot).

    Uses agent-browser (headless Chromium). Handles JavaScript-rendered pages,
    SPAs, and complex web apps. Returns the accessibility tree with @ref selectors
    that can be used with browser_extract.

    Best for: JavaScript-rendered pages, SPAs, pages needing login, complex web apps.
    Slower than web_fetch but handles everything.

    Args:
        url: The full URL to navigate to.
        wait_ms: Milliseconds to wait for page load (default 5000).
    """
    if not url.startswith(("http://", "https://")):
        url = "https://" + url
    import time
    # Open page
    nav = _browser_cmd(["open", url], timeout_sec=30)
    if nav.startswith("("):
        return nav
    # Let page settle
    time.sleep(min(wait_ms / 1000, 3))
    # Get snapshot + title
    snapshot = _browser_cmd(["snapshot", "-i", "--max-output", "15000"], timeout_sec=20)
    title = _browser_cmd(["get", "title"], timeout_sec=5)
    title_str = title if "empty" not in title.lower() and "error" not in title.lower() else ""
    return f"[BROWSER] {url}\nTitle: {title_str}\n\nAccessibility Tree:\n{snapshot[:15000]}"


def _browser_cmd(args: list[str], timeout_sec: int = 30) -> str:
    """Run an agent-browser CLI command. Daemon starts/stops automatically."""
    import subprocess, os
    agent_browser = "agent-browser"
    if os.name == "nt":
        npm_cmd = os.path.expanduser(r"~\AppData\Roaming\npm\agent-browser.cmd")
        if os.path.isfile(npm_cmd):
            agent_browser = npm_cmd
    try:
        r = subprocess.run([agent_browser] + args, capture_output=True, text=True, timeout=timeout_sec)
        return r.stdout.strip() or "(empty)"
    except FileNotFoundError:
        return "(agent-browser not found. Run: npm i -g agent-browser)"
    except subprocess.TimeoutExpired:
        return f"(timed out after {timeout_sec}s)"
    except Exception as e:
        return f"(error: {e})"


def browser_extract(what: str = "text", selector: str = "") -> str:
    """Extract content from the current browser page.

    Args:
        what: What to extract â€” 'text' (default), 'html', 'value', 'title', 'url'.
        selector: Optional CSS selector or @ref from snapshot. Empty = entire page.
    """
    valid = {"text", "html", "value", "title", "url", "attr"}
    if what not in valid:
        return f"(invalid: '{what}'. Valid: {', '.join(sorted(valid))})"
    if what in ("title", "url"):
        return _browser_cmd(["get", what], timeout_sec=5)
    if selector:
        return _browser_cmd(["get", what, selector], timeout_sec=15)
    return _browser_cmd(["get", what, "body"], timeout_sec=15)


def browser_screenshot(path: str = "") -> str:
    """Take a screenshot of the current browser page.

    Args:
        path: Optional file path to save the screenshot. Auto-named if empty.
    """
    import os
    if not path:
        import uuid
        screenshot_dir = os.path.join(os.environ.get("TEMP", "."), "agent-screenshots")
        os.makedirs(screenshot_dir, exist_ok=True)
        path = os.path.join(screenshot_dir, f"screenshot_{uuid.uuid4().hex[:8]}.png")
    result = _browser_cmd(["screenshot", "--full", path], timeout_sec=20)
    if os.path.isfile(path):
        size = os.path.getsize(path)
        return f"[SCREENSHOT] Saved to {path} ({size} bytes)"
    return f"(screenshot result: {result})"


def browser_click(selector: str) -> str:
    """Click an element using CSS selector or snapshot @ref tag."""
    return _browser_cmd(["click", selector], timeout_sec=15)


def browser_fill(selector: str, text: str) -> str:
    """Clear and fill an input element."""
    return _browser_cmd(["fill", selector, text], timeout_sec=15)


def browser_type(selector: str, text: str) -> str:
    """Type text into an element (simulates keystrokes)."""
    return _browser_cmd(["type", selector, text], timeout_sec=15)


def browser_press(key: str) -> str:
    """Press a keyboard key (e.g. 'Enter')."""
    return _browser_cmd(["press", key], timeout_sec=10)


def browser_close() -> str:
    """Close the browser daemon and free resources."""
    _browser_cmd(["close", "--all"], timeout_sec=10)
    return "Browser closed."


def browser_act(action: str = "extract", selector: str = "", text: str = "",
                key: str = "", what: str = "text", path: str = "",
                url: str = "", wait_ms: int = 5000) -> str:
    """Unified browser tool: one entry point for navigate / click / fill /
    type / press / extract / screenshot / close. Routes to the individual
    browser primitives so the agent surface stays minimal (one schema, one
    action enum) while all capabilities remain available."""
    a = (action or "extract").strip().lower()
    if a == "navigate":
        return browser_navigate(url, wait_ms)
    if a == "click":
        return browser_click(selector)
    if a == "fill":
        return browser_fill(selector, text)
    if a == "type":
        return browser_type(selector, text)
    if a == "press":
        return browser_press(key)
    if a == "extract":
        return browser_extract(what, selector)
    if a == "screenshot":
        return browser_screenshot(path)
    if a == "close":
        return browser_close()
    return (f"Error: unknown browser_act action '{action}'. "
            f"Valid: navigate, click, fill, type, press, extract, screenshot, close")


# -- GitHub tools ------------------------------------------------------------

def github_create_repo(name: str, private: bool = False, description: str = "") -> str:
    """Create a GitHub repository using the gh CLI."""
    import subprocess
    try:
        args = ["gh", "repo", "create", name, "--" + ("private" if private else "public")]
        if description:
            args += ["-d", description]
        r = subprocess.run(args, capture_output=True, text=True, timeout=30)
        return r.stdout.strip() or f"Created repo: {name}"
    except FileNotFoundError:
        return "Error: gh CLI not installed. Run: winget install GitHub.cli"


def github_create_issue(title: str, body: str = "", repo: str = "") -> str:
    """Create a GitHub issue using the gh CLI."""
    import subprocess
    try:
        args = ["gh", "issue", "create", "-t", title]
        if body:
            args += ["-b", body]
        if repo:
            args += ["-R", repo]
        r = subprocess.run(args, capture_output=True, text=True, timeout=30)
        return r.stdout.strip() or f"Created issue: {title}"
    except FileNotFoundError:
        return "Error: gh CLI not installed."


def github_create_pull_request(title: str, body: str = "", base: str = "main", head: str = "") -> str:
    """Create a GitHub pull request using the gh CLI."""
    import subprocess
    try:
        args = ["gh", "pr", "create", "-t", title, "-B", base]
        if body:
            args += ["-b", body]
        if head:
            args += ["-H", head]
        r = subprocess.run(args, capture_output=True, text=True, timeout=30)
        return r.stdout.strip() or f"Created PR: {title}"
    except FileNotFoundError:
        return "Error: gh CLI not installed."


def github_list_issues(state: str = "open", limit: int = 10) -> str:
    """List GitHub issues using the gh CLI."""
    import subprocess
    try:
        r = subprocess.run(
            ["gh", "issue", "list", "-s", state, "-L", str(limit)],
            capture_output=True, text=True, timeout=30
        )
        return r.stdout.strip() or "No issues found."
    except FileNotFoundError:
        return "Error: gh CLI not installed."

# -- PDF reader ---------------------------------------------------------------

def read_pdf(file_path: str, max_pages: int = 20, page_numbers: str = "") -> str:
    """Read text from a PDF file.

    Args:
        file_path: Path to the PDF file.
        max_pages: Max pages to read (default 20). Use 0 for all.
        page_numbers: Specific pages to read, e.g. '1-3,5,7-9' (overrides max_pages).
    """
    if not os.path.isfile(file_path):
        return "Error: File not found: " + file_path
    if not file_path.lower().endswith('.pdf'):
        return "Error: Not a PDF file: " + file_path
    try:
        import fitz
    except ImportError:
        return "Error: pymupdf not installed. Run: pip install pymupdf"
    try:
        doc = fitz.open(file_path)
        total = len(doc)
        pages = []
        if page_numbers:
            import re
            for part in page_numbers.split(','):
                part = part.strip()
                if '-' in part:
                    a, b = part.split('-', 1)
                    pages.extend(range(int(a.strip())-1, int(b.strip())))
                else:
                    pages.append(int(part)-1)
        else:
            max_p = max_pages if max_pages > 0 else total
            pages = list(range(min(total, max_p)))
        output = []
        output.append("PDF: " + os.path.basename(file_path) + " (" + str(total) + " pages)")
        for pnum in pages:
            if pnum >= total:
                break
            page = doc[pnum]
            text = page.get_text().strip()
            if text:
                output.append("--- Page " + str(pnum+1) + " ---")
                # Truncate very long pages to 3000 chars
                if len(text) > 3000:
                    text = text[:1500] + "\n...[truncated]...\n" + text[-1500:]
                output.append(text)
        doc.close()
        result = "\n".join(output)
        if len(result) > 50000:
            result = result[:25000] + "\n...[large PDF truncated]...\n" + result[-25000:]
        return result if result.strip() else "(empty PDF)"
    except Exception as e:
        return "Error reading PDF: " + str(e)


# -- Document creation & web search tools -----------------------------------

def create_docx(file_path, title="", content=""):
    """Create a Word document (.docx) with optional title and content."""
    try:
        from docx import Document
    except ImportError:
        return "Error: python-docx not installed. Run: pip install python-docx"
    try:
        doc = Document()
        if title:
            doc.add_heading(title, level=1)
        for para in content.split(chr(10)):
            para = para.strip()
            if para:
                doc.add_paragraph(para)
        os.makedirs(os.path.dirname(file_path) or ".", exist_ok=True)
        doc.save(file_path)
        return "Created Word document: " + file_path
    except Exception as e:
        return "Error creating document: " + str(e)


def create_xlsx(file_path, headers=None, rows=None):
    """Create an Excel spreadsheet (.xlsx)."""
    try:
        from openpyxl import Workbook
    except ImportError:
        return "Error: openpyxl not installed. Run: pip install openpyxl"
    try:
        wb = Workbook()
        ws = wb.active
        headers = headers or []
        rows = rows or []
        if headers:
            ws.append(headers)
        for row in rows:
            ws.append(row if isinstance(row, list) else [row])
        os.makedirs(os.path.dirname(file_path) or ".", exist_ok=True)
        wb.save(file_path)
        return "Created Excel: " + file_path + " (" + str(len(headers)) + " cols, " + str(len(rows)) + " rows)"
    except Exception as e:
        return "Error creating spreadsheet: " + str(e)


def web_search(query, max_results=5):
    """Search the web via DuckDuckGo."""
    import urllib.parse
    encoded = urllib.parse.quote(query)
    url = "https://html.duckduckgo.com/html/?q=" + encoded
    try:
        result = web_fetch(url, max_chars=8000)
        import re
        snippets = re.findall(
            r'<a[^>]*class="result__a"[^>]*>(.*?)</a>.*?'
            r'<a[^>]*class="result__snippet"[^>]*>(.*?)</a>',
            result, re.DOTALL
        )
        if snippets:
            output = 'Web search: ' + chr(34) + query + chr(34)
            for i, (title, snippet) in enumerate(snippets[:max_results], 1):
                clean_title = re.sub(r'<[^>]+>', '', title).strip()
                clean_snippet = re.sub(r'<[^>]+>', '', snippet).strip()
                output += chr(10) + str(i) + '. ' + clean_title + chr(10) + '   ' + clean_snippet
            return output
        return "Search: " + chr(34) + query + chr(34) + chr(10) + result[:5000]
    except Exception as e:
        return "Search error: " + str(e)


def update_feature(feature_name: str = "", name: str = "", description: str = "", file_refs: list = None) -> str:
    """Add/update a feature entry in long-term memory (features.md). Thin wrapper over memory_manager."""
    try:
        from memory_manager import update_feature as _mem_feature
        ws = shared_state.get("project_path") or WORKSPACE
        fname = feature_name or name
        if not fname:
            return "Error: update_feature requires 'name' (or 'feature_name')."
        ok = _mem_feature(ws, fname, description, file_refs or [])
        return f"[OK] Feature '{fname}' updated in long-term memory." if ok else f"[ERR] Could not update feature '{fname}'."
    except Exception as e:
        return f"Error updating feature: {e}"


def update_project_meta(key: str = "", value: str = "") -> str:
    """Add/update a project metadata section in long-term memory (project.md). Thin wrapper over memory_manager."""
    try:
        from memory_manager import update_project_meta as _mem_meta
        ws = shared_state.get("project_path") or WORKSPACE
        if not key:
            return "Error: update_project_meta requires 'key'."
        ok = _mem_meta(ws, key, value)
        allowed = "Tech Stack, Frontend, Database, Testing, Architecture, Project Type, Goals"
        return f"[OK] Project metadata '{key}' updated." if ok else f"[ERR] Unknown metadata key '{key}' (allowed: {allowed})."
    except Exception as e:
        return f"Error updating project metadata: {e}"


# ── C11: File snapshots + revert primitive ──
# Before any write/edit mutation, the prior content is snapshotted to
# <workspace>/.deep_agents/snapshots/. revert_last_change() restores the latest.
# Gated by BSA_SNAPSHOT (default on). Storage bounded to 5 snapshots per file.

def _snapshot_dir() -> str:
    import uuid
    ws = os.path.abspath(shared_state.get("project_path") or WORKSPACE or ".")
    snap_dir = os.path.join(ws, ".deep_agents", "snapshots")
    os.makedirs(snap_dir, exist_ok=True)
    return snap_dir


def _snapshot_file(file_path: str) -> str:
    """Snapshot existing file content before a mutation. Returns snapshot path or ''."""
    if os.environ.get("BSA_SNAPSHOT", "1") != "1":
        return ""
    try:
        import uuid
        import glob
        import hashlib
        norm = os.path.normpath(os.path.abspath(file_path))
        if not os.path.isfile(norm):
            return ""
        with open(norm, "r", encoding="utf-8", errors="replace") as f:
            content = f.read()
        digest = hashlib.sha256(norm.lower().encode("utf-8", "replace")).hexdigest()[:12]
        fname = "".join(c if c.isalnum() or c in "-_." else "_" for c in os.path.basename(norm))[:40] or "file"
        snap_path = os.path.join(
            _snapshot_dir(),
            f"{digest}_{fname}_{time.strftime('%Y%m%d_%H%M%S')}_{uuid.uuid4().hex[:4]}.snap",
        )
        with open(snap_path, "w", encoding="utf-8") as f:
            f.write(content)
        # Bound storage: keep the 5 most recent snapshots per file digest
        try:
            olds = sorted(glob.glob(os.path.join(_snapshot_dir(), f"{digest}_*.snap")))
            for old in olds[:-5]:
                os.remove(old)
        except Exception:
            pass
        return snap_path
    except Exception:
        return ""


def revert_last_change(file_path: str = "") -> str:
    """Restore the most recent snapshot of a file (undo the last write/edit)."""
    try:
        import glob
        import hashlib
        if not file_path:
            return "Error: revert_last_change requires 'file_path'."
        norm = os.path.normpath(os.path.abspath(file_path))
        digest = hashlib.sha256(norm.lower().encode("utf-8", "replace")).hexdigest()[:12]
        snaps = sorted(glob.glob(os.path.join(_snapshot_dir(), f"{digest}_*.snap")))
        if not snaps:
            return f"Error: No snapshots found for {file_path}."
        latest = snaps[-1]
        with open(latest, "r", encoding="utf-8") as f:
            content = f.read()
        os.makedirs(os.path.dirname(norm) or ".", exist_ok=True)
        with open(norm, "w", encoding="utf-8") as f:
            f.write(content)
        os.remove(latest)
        _invalidate_file_cache(norm)
        return f"[OK] Reverted {file_path} to snapshot {os.path.basename(latest)} ({len(content)} chars restored)."
    except Exception as e:
        return f"Error reverting {file_path}: {e}"


# ── A4: Salvage-on-death ──
def capture_workspace_delta(max_chars: int = 6000) -> str:
    """Best-effort git diff + status of the active workspace for salvage-on-death.

    Called on hard-stop / safety-ceiling so a dead agent still delivers its
    partial work. Returns '' when the workspace is not a git repo.
    """
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


# ── C10: Parallel re-attempts for failed tasks ──
def launch_parallel_attempts(name: str, task_desc: str, n: int = 2, wait_seconds: int = 300) -> str:
    """Run N independent parallel re-attempts of a task; return the best result.

    Uses the existing async pool (20 workers). Picks the attempt whose report is
    most complete and least failure-marked. Gated by BSA_PARALLEL_RETRY (default off).
    """
    import time as time_mod
    import re as _re
    if os.environ.get("BSA_PARALLEL_RETRY", "0") != "1":
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


TOOL_MAP = {
    "read_file": read_file,
    "write_file": write_file,
    "edit_file": edit_file,
    "apply_diff": lambda file_path, diff, **kw: edit_file(file_path, diff=diff),
    "run_command": run_command,
    "search_code": search_code,
    "list_files": list_files,
    "view_signatures": view_signatures,
    "run_js": run_js,
    "query_history": query_history,
    "compact_conversation": compact_conversation,
    "task": task,
    "start_async_task": start_async_task,
    "check_async_task": check_async_task,
    "list_async_tasks": list_async_tasks,
    "pipeline_tools": pipeline_tools,
    "search_semantic_checkpoints": search_semantic_checkpoints,
    "search_codebase": search_codebase,
    "web_fetch": web_fetch,
    "browser_navigate": browser_navigate,
    "browser_act": browser_act,
    "browser_extract": browser_extract,
    "browser_screenshot": browser_screenshot,
    "browser_click": browser_click,
    "browser_fill": browser_fill,
    "browser_type": browser_type,
    "browser_press": browser_press,
    "browser_close": browser_close,
    "github_create_repo": github_create_repo,
    "github_create_issue": github_create_issue,
    "github_create_pull_request": github_create_pull_request,
    "github_list_issues": github_list_issues,
    "create_docx": create_docx,
    "create_xlsx": create_xlsx,
    "read_pdf": read_pdf,
    "web_search": web_search,
    "update_feature": update_feature,
    "update_project_meta": update_project_meta,
    "revert_last_change": revert_last_change,
}


def execute_tool(name: str, args: dict) -> str:
    """Execute a tool by name with the given arguments. Returns the result string."""
    if name not in TOOL_MAP:
        return f"Error: Unknown tool '{name}'. Available tools: {', '.join(TOOL_MAP.keys())}"
    try:
        # Normalize and map common parameter variations based on tool signature
        import inspect
        func = TOOL_MAP[name]
        try:
            sig = inspect.signature(func)
            param_names = list(sig.parameters.keys())
        except Exception:
            param_names = []
            
        args = dict(args)
        
        # 1. Map 'path' or 'filepath' -> 'file_path' if tool expects 'file_path'
        if "file_path" in param_names and "file_path" not in args:
            for alt in ("path", "filepath", "file"):
                if alt in args:
                    args["file_path"] = args.pop(alt)
                    break
                    
        # 2. Map 'cmd' -> 'command' if tool expects 'command'
        if "command" in param_names and "command" not in args:
            if "cmd" in args:
                args["command"] = args.pop("cmd")
                
        # 3. Map subagent task parameter variations
        if "name" in param_names and "name" not in args:
            for alt in ("agent", "subagent"):
                if alt in args:
                    args["name"] = args.pop(alt)
                    break
        if "task" in param_names and "task" not in args:
            for alt in ("instruction", "instructions", "prompt", "body"):
                if alt in args:
                    args["task"] = args.pop(alt)
                    break

        # â”€â”€ Pillar 113: Check read-only tool response cache â”€â”€
        cached = _check_tool_cache(name, args)
        if cached is not None:
            return cached

        result = func(**args)

        # â”€â”€ Store in cache for read-only tools â”€â”€
        if isinstance(result, str) and not result.startswith("Error"):
            _set_tool_cache(name, args, result)
        
        # Auto-offload extremely large tool outputs (over 80,000 chars ~ 20,000 tokens)
        if isinstance(result, str) and len(result) > 80000 and name != "read_file":
            import uuid
            file_id = uuid.uuid4().hex[:8]
            vfs_path = f"/scratch/large_output_{file_id}.txt"
            
            # Save the full result to the scratch VFS path
            write_file(vfs_path, result)
            
            # Budgeted head+tail preview (errors/assertions live in the tail)
            orig_result_len = len(result)
            preview = _create_budgeted_preview(result)
            
            result = (
                f"[TOOL OUTPUT OFFLOADED to VFS path: {vfs_path}]\n"
                f"Total lines: {len(result.splitlines())}, Total characters: {orig_result_len}\n"
                f"--- PREVIEW (head+tail) ---\n"
                f"{preview}\n"
                f"---------------------------\n"
                f"[Use read_file with offset/limit to read the offloaded file at {vfs_path} if needed.]"
            )
            try:
                from context_compaction import truncation_note
                result += truncation_note(orig_result_len, 80000)
            except Exception:
                pass
        return result
    except TypeError as e:
        return f"Error: Invalid arguments for {name}: {e}"
    except Exception as e:
        return f"Error executing {name}: {e}"


# â”€â”€ Pillar 80: Speculative Tool Pre-Parsing â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

# Required arg names for each tool (loose validation to catch obvious mistakes)
_TOOL_REQUIRED_ARGS: dict[str, set[str]] = {
    "read_file": {"file_path"},
    "write_file": {"file_path", "content"},
    "edit_file": {"file_path"},
    "apply_diff": {"file_path", "diff"},
    "run_command": {"command"},
    "search_code": {"pattern"},
    "list_files": set(),
    "web_fetch": {"url"},
    "browser_navigate": {"url"},
    "browser_act": {"action"},
    "browser_extract": set(),
    "browser_screenshot": set(),
    "browser_close": set(),
}

def pre_validate_tool_args(tool_name: str, args: dict) -> tuple[bool, str]:
    """
    Pre-validate tool call arguments before execution.
    Checks required args are present and arg types look reasonable.
    Returns (is_valid, error_message).
    Early rejection saves failed execution attempts.
    """
    if tool_name not in _TOOL_REQUIRED_ARGS:
        return True, ""  # Unknown tool â€” let execute_tool handle it

    required = _TOOL_REQUIRED_ARGS[tool_name]
    missing = required - set(args.keys())
    if missing:
        return False, f"Missing required args for '{tool_name}': {', '.join(sorted(missing))}"

    # Type sanity checks
    for key in ("file_path", "path", "pattern", "command", "content", "diff"):
        if key in args and not isinstance(args[key], str):
            return False, f"Expected string for arg '{key}' in '{tool_name}', got {type(args[key]).__name__}"

    for key in ("timeout", "offset", "limit", "background"):
        if key in args and args[key] is not None and not isinstance(args[key], (int, float, bool)):
            return False, f"Expected number/bool for arg '{key}' in '{tool_name}', got {type(args[key]).__name__}"

    return True, ""

