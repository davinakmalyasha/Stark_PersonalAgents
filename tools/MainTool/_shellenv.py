"""MainTool._shellenv — bash discovery and the per-process shell environment.

Performance model (unchanged from the original, now isolated here):
  * bash path + venv Scripts dir + GH_TOKEN/.env resolution are discovered
    ONCE per process and cached — a run_command no longer re-scans the
    filesystem or re-parses .env on every invocation.
  * A persistent BASH_ENV file (written once) exports shell hygiene vars into
    every `bash -lc` call, so commands inherit the same environment with zero
    per-call setup cost.
  * Credentials are injected per call, only for commands that authenticate
    (gh/git/curl/package managers). They are deliberately NOT in BASH_ENV:
    an arbitrary npm script or test must not be able to read GH_TOKEN.
"""
from __future__ import annotations

import os
import threading
from typing import Optional

from ._cmdpolicy import needs_credentials

_DISCOVERY_LOCK = threading.Lock()

_BASH_PATH_CACHE: Optional[str] = None
_BASH_PATH_SOURCE: Optional[str] = None  # env override that produced the cache
_VENV_CACHE: Optional[str] = None
_VENV_KEY_CACHE: Optional[str] = None

# Base environment: os.environ snapshot + PATH prepend. Built once; per-call
# env = copy of this + scoped credentials.
_BASE_ENV_CACHE: Optional[dict] = None
_ENV_SOURCE_HASH: Optional[int] = None  # os.environ id() to detect wholesale swaps

# Persistent BASH_ENV file: exports the resolved environment into every bash -lc.
_BASH_ENV_FILE: Optional[str] = None


def _find_bash() -> Optional[str]:
    """Locate a POSIX bash for run_command. Prefers git-bash (ships with Git for
    Windows); rejects the WSL shim (System32\\bash.exe) which only works if WSL
    is installed. Result is cached per process (invalidate when BASH_PATH changes)."""
    global _BASH_PATH_CACHE, _BASH_PATH_SOURCE
    env_bash = os.environ.get("BASH_PATH", "").strip()
    if env_bash != _BASH_PATH_SOURCE:
        # Env override changed (or first call): invalidate the cache.
        _BASH_PATH_CACHE = None
        _BASH_PATH_SOURCE = env_bash
    if _BASH_PATH_CACHE is not None:
        return _BASH_PATH_CACHE

    with _DISCOVERY_LOCK:
        if _BASH_PATH_CACHE is not None:
            return _BASH_PATH_CACHE
        found = None
        if env_bash and os.path.isfile(env_bash):
            found = env_bash
        if found is None:
            candidates = [
                os.path.join(os.environ.get("ProgramFiles", r"C:\Program Files"), "Git", "bin", "bash.exe"),
                os.path.join(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"), "Git", "bin", "bash.exe"),
                os.path.join(os.environ.get("LocalAppData", ""), "Programs", "Git", "bin", "bash.exe"),
            ]
            for c in candidates:
                if c and os.path.isfile(c):
                    found = c
                    break
        if found is None:
            import shutil
            w = shutil.which("bash")
            if w:
                low = w.lower()
                if "system32" not in low and "syswow64" not in low:
                    found = w
        _BASH_PATH_CACHE = found
        return _BASH_PATH_CACHE


def _resolve_venv_scripts(root_dir: str) -> Optional[str]:
    """Find the active venv Scripts/bin dir once (cached). Order: VOICE_PYTHON,
    then <root>/venv > .venv > venv312, then harness-root equivalents."""
    global _VENV_CACHE, _VENV_KEY_CACHE
    if _VENV_CACHE is not None:
        return _VENV_CACHE
    voice_py = os.environ.get("VOICE_PYTHON", "").strip()
    if voice_py and os.path.isfile(voice_py):
        return os.path.dirname(voice_py)
    # The harness root is this repo (tools/MainTool/_shellenv.py -> 3 dirnames
    # up = repo root). tools/__init__.py exports a DIFFERENT _HARNESS_ROOT
    # (the tools dir), so never import it here — compute from __file__.
    repo_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    roots = [root_dir, repo_root, os.environ.get("HARNESS_ROOT", "")]
    seen = set()
    for base in roots:
        if not base:
            continue
        base = os.path.normpath(base)
        if base.lower() in seen:
            continue
        seen.add(base.lower())
        for venv_name in ["venv", ".venv", "venv312"]:
            for folder in ["Scripts", "bin"]:
                candidate = os.path.join(base, venv_name, folder)
                if os.path.isdir(candidate):
                    _VENV_CACHE = candidate
                    _VENV_KEY_CACHE = "PATH"
                    return _VENV_CACHE
    return None


def _base_env() -> dict:
    """Build the per-process base environment once: os.environ snapshot +
    venv PATH prepend. Credentials are resolved per call, not here."""
    global _BASE_ENV_CACHE, _ENV_SOURCE_HASH
    if _BASE_ENV_CACHE is not None and _ENV_SOURCE_HASH == id(os.environ):
        return _BASE_ENV_CACHE
    with _DISCOVERY_LOCK:
        if _BASE_ENV_CACHE is not None and _ENV_SOURCE_HASH == id(os.environ):
            return _BASE_ENV_CACHE
        env = dict(os.environ)
        venv = _resolve_venv_scripts(os.environ.get("HARNESS_ROOT") or "")
        if venv:
            path_key = "PATH"
            for k in list(env.keys()):
                if k.upper() == "PATH":
                    path_key = k
                    break
            existing = env.get(path_key, "")
            env[path_key] = (venv + os.path.pathsep + existing) if existing else venv
        _BASE_ENV_CACHE = env
        _ENV_SOURCE_HASH = id(os.environ)
        return _BASE_ENV_CACHE


def _resolve_gh_token() -> Optional[str]:
    """GH_TOKEN from the process env, else GITHUB_TOKEN, else .env (parsed once)."""
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token:
        return token
    try:
        from dotenv import load_dotenv
        load_dotenv()
        return os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    except Exception:
        return None


def bash_args() -> list:
    """Args for invoking bash with one command string.

    Non-login ``-c`` is the default: Git Bash's ``/etc/profile`` costs ~900ms
    per call on some machines and can emit MSYS post-install permission noise.
    Everything the profile provided (PATH, HOME) is already resolved here or by
    bash itself. Set AGENT_BASH_LOGIN=1 to restore ``-lc`` login behavior.
    """
    if os.environ.get("AGENT_BASH_LOGIN", "0").lower() in ("1", "true", "yes"):
        return ["-lc"]
    return ["-c"]


def _bash_env_file() -> Optional[str]:
    """Create (once) the persistent BASH_ENV file. Every `bash -lc` inherits
    these via BASH_ENV, so hygiene vars persist across calls without per-call
    setup. Credentials are intentionally excluded (scoped per call instead).
    Disable with AGENT_BASH_ENV=0."""
    global _BASH_ENV_FILE
    if _BASH_ENV_FILE is not None:
        return _BASH_ENV_FILE
    if os.environ.get("AGENT_BASH_ENV", "1").lower() in ("0", "false", "no"):
        return None
    lines = [
        "export PYTHONUNBUFFERED=1",
        "export PIP_DISABLE_PIP_VERSION_CHECK=1",
        "export PYTHONDONTWRITEBYTECODE=1",
        "export LC_ALL=C.UTF-8 2>/dev/null || true",
    ]
    try:
        import tempfile
        fd, path = tempfile.mkstemp(prefix="agent_bash_env_", suffix=".sh")
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
        try:
            os.chmod(path, 0o444)  # read-only: agents can't mutate the env file
        except Exception:
            pass
        _BASH_ENV_FILE = path
    except Exception:
        _BASH_ENV_FILE = None
    return _BASH_ENV_FILE


def per_call_env(command: str = "") -> dict:
    """Cheap per-call environment: base copy + credentials only when needed.

    ``command`` is the raw shell command; credentials are injected when it
    references an authenticating program (gh/git/curl/npm/pip/...). Without a
    command, credentials are stripped to the os.environ baseline.
    """
    env = dict(_base_env())
    if command and needs_credentials(command):
        token = _resolve_gh_token()
        if token:
            env["GH_TOKEN"] = token
            env.setdefault("GITHUB_TOKEN", token)
    else:
        env.pop("GH_TOKEN", None)
        env.pop("GITHUB_TOKEN", None)
    # Git-Bash/MSYS converts POSIX-looking arguments to Windows paths when it
    # launches native programs, which mangles things like `/api/x` or `-H /foo`.
    # Opt out with AGENT_MSYS_NO_PATHCONV=1 (MSYS2_ARG_CONV_EXCL passes through
    # from os.environ automatically).
    if os.environ.get("AGENT_MSYS_NO_PATHCONV", "").lower() in ("1", "true", "yes"):
        env["MSYS_NO_PATHCONV"] = "1"
    return env
