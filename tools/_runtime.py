"""Shared tools runtime: permissions, VFS, caches, and output offload.

This module owns the state that the tools.py facade re-exports. It is kept
independent of the facade so individual tool modules can import the state
lazily without creating an import cycle.
"""
import hashlib
import os
import json
import threading
import time
from contextvars import ContextVar
from typing import Optional, Any

from sync.state_sync import shared_state
try:
    from deepagents.middleware.filesystem import FilesystemPermission, _check_fs_permission
except ImportError:
    class FilesystemPermission:  # type: ignore[no-redef]
        def __init__(self, operations=None, paths=None, mode="allow"):
            self.operations = operations or []
            self.paths = paths or []
            self.mode = mode
    def _check_fs_permission(*a, **kw):  # type: ignore[no-redef]
        return True

active_permissions: ContextVar[Optional[list[FilesystemPermission]]] = ContextVar("active_permissions", default=None)

#: Per-agent output caps (chars) set by the spawner from the agent registry.
#: Consumed by _tool_cap; None = global defaults.
active_tool_caps: ContextVar[Optional[dict]] = ContextVar("active_tool_caps", default=None)

#: One member's materialised allowlist (``<project>/tools/bindings.json``),
#: set by the group runner for the duration of that member's run. None = no
#: narrowing beyond the scope ACL.
#:
#: This is what makes the ``never`` tier real (``04`` §2): a tool absent here
#: is absent from the member's toolset altogether, so there is no call to
#: refuse. ``blocked`` is the other half — the tool exists and is refused when
#: used, which is a visible attempt rather than an impossible one.
active_allowed_tools: ContextVar[Optional[frozenset]] = ContextVar("active_allowed_tools", default=None)

_HARNESS_ROOT = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _active_workspace(deps: Any = None) -> str:
    """Return the active workspace root. Prefers the typed per-run deps when
    provided, else falls back to the legacy global shared_state path.
    """
    if deps is not None:
        try:
            p = getattr(deps, "project_path", None)
            if p:
                return str(p)
        except Exception:
            pass
    return shared_state.get("project_path") or WORKSPACE


def to_virtual_path(path: str, deps: Any = None) -> str:
    path_str = path.replace("\\", "/")
    if path_str.startswith(("/workspace/", "/scratch/", "/memories/", "/conversation_history/")):
        return "/" + path_str.lstrip("/")
    if path_str in ("/workspace", "/scratch", "/memories", "/conversation_history"):
        return "/" + path_str.lstrip("/")

    # Resolve physical absolute path
    active_ws = _active_workspace(deps)
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
    from sync.state_sync import active_store
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
    from sync.state_sync import active_store
    store = active_store.get()
    if store is not None:
        try:
            current = _get_cache_version()
            store.put(("tool_cache",), "version", current + 1)
        except Exception:
            pass

WORKSPACE = _HARNESS_ROOT

_TOOL_CAP_DEFAULTS = {
    # Character caps (not bytes), applied centrally in execute_tool_detailed.
    # Full output past a cap is offloaded to /scratch/large_tool_results/.
    "read_file": 50000,
    "search_code": 16000,
    "ast_search": 16000,
    "edit_file": 10000,
    "run_command": 30000,
    "run_command_timeout": 30000,
    "logs": 30000,
    "browser_act": 24000,
    "web_search": 10000,
    "ask_user": 4000,
    "github_apply_gate": 4000,
    "update_plan": 2000,
    "subagent": 32000,
    "mcp": 25000,
}

_CAP_KEY_ALIASES = {
    "call_subagent": "subagent",
    "delegate": "subagent",
    "browser_navigate": "browser_act",
}

def _tool_cap(site: str) -> int:
    key = site.lower()
    if key.startswith("mcp__") or key.startswith("mcp_"):
        key = "mcp"
    key = _CAP_KEY_ALIASES.get(key, key)
    default = _TOOL_CAP_DEFAULTS.get(key, 16000)
    env_name = f"CAP_{key.upper()}"
    env_val = os.environ.get(env_name)
    if env_val is not None:
        try:
            return int(env_val)
        except Exception:
            pass
    caps = active_tool_caps.get()
    if caps:
        v = caps.get(key)
        try:
            if v is not None:
                return int(v)
        except Exception:
            pass
    return default

def _create_budgeted_preview(content_str: str, max_chars: int = 8000,
                             head_ratio: float = 0.25, min_lines: int = 4) -> str:
    """Token-budgeted head+tail preview for offloaded tool output.

    Deterministic per-call cost (no 1000-char-line surprise): at most ~max_chars
    chars of preview, split head 25% / tail 75% (errors, tracebacks and last
    assertions live in the tail; the head only needs command context). Individual
    lines are trimmed to 512 chars. Line-numbered so offset/limit paging stays
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
    head = [ln[:512] for ln in head]
    tail = [ln[:512] for ln in tail]
    omitted = len(lines) - len(head) - len(tail)
    head_sample = format_content_with_line_numbers(head, start_line=1)
    tail_sample = format_content_with_line_numbers(tail, start_line=len(lines) - len(tail) + 1)
    truncation_notice = f"\n... [{omitted} lines truncated] ...\n"
    return head_sample + truncation_notice + tail_sample

def _write_offload_file(content_str: str, tool_name: str) -> str:
    """Persist full tool output under /scratch/large_tool_results/; '' on failure."""
    import uuid

    file_id = uuid.uuid4().hex[:8]
    clean_name = "".join(c if c.isalnum() or c in "-_" else "_" for c in tool_name)[:50]
    vfs_path = f"/scratch/large_tool_results/{clean_name}_{file_id}.txt"
    try:
        vfs_router = get_vfs_router()
        res = vfs_router.write(vfs_path, content_str)
        if res and res.error:
            raise RuntimeError(res.error)
    except Exception:
        # Fallback to writing directly to physical scratch directory
        try:
            path = _sanitize_path(vfs_path)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as f:
                f.write(content_str)
        except Exception as e_inner:
            print(f"Error offloading tool message: {e_inner}")
            return ""
    return vfs_path


def auto_offload_result(content_str: str, tool_name: str, max_chars: int = 16000) -> str:
    """If output is too large, offloads full content to VFS scratch and returns TOO_LARGE_TOOL_MSG.

    `max_chars` is per-call. Large command/test output is mostly noise (progress bars,
    npm warnings, directory dumps) â€” the head (command context) and tail (exit code,
    traceback, last assertion) carry the signal. Offloading early keeps the per-turn
    cache-miss input small without losing the full output (it is on disk, readable).
    """
    if len(content_str) <= max_chars:
        return content_str
    from deepagents.middleware._message_eviction import TOO_LARGE_TOOL_MSG

    vfs_path = _write_offload_file(content_str, tool_name)
    if not vfs_path:
        return content_str[:max_chars] + "\n... [TRUNCATED — offload write failed] ..."

    content_sample = _create_budgeted_preview(content_str, max(1, min(8000, max_chars)))
    try:
        from memory.context_compaction import truncation_note
        content_sample += truncation_note(len(content_str), max_chars)
    except Exception:
        pass
    return TOO_LARGE_TOOL_MSG.format(
        tool_call_id=os.path.basename(vfs_path)[:-4],
        file_path=vfs_path,
        content_sample=content_sample
    )

_FILE_CACHE: dict[str, tuple[float, str]] = {}

_RUN_OUTPUT_CACHE: dict[str, str] = {}

_TOOL_RESPONSE_CACHE: dict[str, tuple[float, str]] = {}

_TOOL_MTIME_MAP: dict[str, float] = {}

_TOOL_CACHE_TTL = 300  # seconds

# Parallel read-only tools run on a thread pool; these dicts are shared across
# those threads, so every read-modify-write goes through one lock.
_CACHE_LOCK = threading.RLock()

def _make_tool_cache_key(tool_name: str, args: dict) -> str:
    """Create a deterministic cache key from tool name and sorted args."""
    raw = tool_name + ":" + json.dumps(args, sort_keys=True, default=str)
    return hashlib.sha256(raw.encode()).hexdigest()

def _check_tool_cache(tool_name: str, args: dict) -> Optional[str]:
    """Check if a read-only tool call has a fresh cached result. Returns None on miss."""
    if tool_name not in ("read_file", "search_code"):
        return None
    now = time.time()
    key = _make_tool_cache_key(tool_name, args)
    with _CACHE_LOCK:
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
    if tool_name not in ("read_file", "search_code"):
        return
    key = _make_tool_cache_key(tool_name, args)
    with _CACHE_LOCK:
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
    with _CACHE_LOCK:
        _TOOL_RESPONSE_CACHE.clear()
        _TOOL_MTIME_MAP.clear()

def _invalidate_file_cache(path: str) -> None:
    """Invalidate cache entry when a file is modified."""
    key = os.path.normpath(path).lower()
    with _CACHE_LOCK:
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

def get_vfs_router(deps: Any = None) -> Any:
    from deepagents.backends import CompositeBackend, StateBackend, FilesystemBackend, StoreBackend
    from sync.state_sync import active_store

    active_ws = _active_workspace(deps)
    active_ws = os.path.normpath(os.path.abspath(active_ws))

    store = active_store.get()
    # deepagents 0.7 requires namespace factory (outside-graph direct use)
    _mem_ns = lambda _rt: ("memories",)
    store_backend = StoreBackend(namespace=_mem_ns, store=store) if store is not None else StoreBackend(namespace=_mem_ns)

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

def _sanitize_path_fallback(path: str, deps: Any = None) -> str:
    active_ws = _active_workspace(deps)
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

def _sanitize_path(path: str, deps: Any = None) -> str:
    """Resolve a relative or absolute path within allowed roots, using VFS routing."""
    from deepagents.backends import FilesystemBackend
    
    vfs_router = get_vfs_router(deps)
    resolved_path = None
    try:
        backend, stripped_key = vfs_router._get_backend_and_key(path)
        if isinstance(backend, FilesystemBackend):
            resolved_path = str(backend._resolve_path(stripped_key))
    except Exception:
        pass
        
    if resolved_path is None:
        resolved_path = _sanitize_path_fallback(path, deps)
        
    real_path = os.path.normpath(os.path.abspath(resolved_path))
    
    # Block access to internal metadata folder (.deep_agents / .antigravity) except when reading scratch/memories/chats/history
    path_parts = real_path.replace("\\", "/").split("/")
    lower_parts = [p.lower() for p in path_parts]
    if ".deep_agents" in lower_parts or ".antigravity" in lower_parts:
        if "scratch" not in path_parts and "memories" not in path_parts and "chats" not in path_parts and "conversation_history" not in path_parts:
            raise ValueError("Access to internal metadata folder is restricted.")
        
    # Security: strictly allow only the active workspace, LangChain system directory, scratch, memories, and history
    active_ws = _active_workspace(deps)
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

def _check_and_request_approval(file_path: str, action: str, deps: Any = None):
    import os
    norm_path = os.path.normpath(file_path).lower()
    base_name = os.path.basename(norm_path)

    # Core settings files should only be blocked if they are modified inside the main LangChain codebase folder.
    # Subagents executing inside sandbox workspaces (e.g. TestProjectForAgent) must be allowed to create/edit package.json, etc.
    active_ws = _active_workspace(deps)
    active_ws_norm = os.path.normpath(active_ws).lower()
    # Harness root — env-overridable, defaults to this repo's directory (portable)
    langchain_root = _HARNESS_ROOT
    langchain_root = os.path.normpath(langchain_root).lower()
    
    in_langchain = (
        norm_path.startswith(langchain_root)
        or (not norm_path.startswith("c:") and not norm_path.startswith("d:")) # Relative paths are usually inside active project
        and langchain_root in active_ws_norm
    )

    # Eval/benchmark workspaces under <root>/scratch/eval_tmp are throwaway
    # sandboxes: their config files (config.json, package.json, ...) are the
    # TASK ITSELF and must be freely editable. The approval gate is for the
    # real codebase, not benchmark scratch dirs.
    is_eval_ws = norm_path.startswith(
        os.path.normpath(os.path.join(langchain_root, "scratch", "eval_tmp")).lower()
    )
    if is_eval_ws:
        in_langchain = False

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
