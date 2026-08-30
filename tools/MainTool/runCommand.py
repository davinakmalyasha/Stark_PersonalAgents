"""MainTool.runCommand — the run_command tool and its shell backend.

Holds run_command, LocalShellBackend, the blocked-command policy
(BLOCKED_PATTERNS + env-extensible deny list), bash discovery, and the
multiline `python -c` rewrite. tools.py state helpers (_HARNESS_ROOT,
_FILE_CACHE, _increment_cache_version, shared_state, WORKSPACE,
auto_offload_result, _tool_cap, _RUN_OUTPUT_CACHE) are imported lazily
inside the functions to avoid an import cycle.

Performance model (per-call hot path):
  * bash path + venv Scripts dir + GH_TOKEN/.env resolution are discovered
    ONCE per process and cached — a run_command no longer re-scans the
    filesystem or re-parses .env on every invocation.
  * A persistent BASH_ENV file (written once) exports the resolved PATH,
    tokens, and shell hygiene vars into every `bash -lc` call, so the model's
    commands inherit the same environment with zero per-call setup cost.
  * Foreground output is appended to the live log in ~200ms chunks instead of
    per line — one state_version bump + one string copy per chunk instead of
    per line (the SSE streamer polls state_version, so batching is invisible
    to the frontend).
  * Background spawns do a quick 0.2s liveness probe instead of a 1.5s sleep.
"""
import collections
import os
import re
import subprocess
import threading
import queue
import time
from typing import Optional
from sync.state_sync import safe_append_live_log
from ._shared import _clean_terminal_output, _log_tool


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

# C12: env-extensible deny list — comma-separated extra patterns (CMD_DENY_EXTRA)
def _load_extra_deny_patterns() -> set:
    extra = os.environ.get("CMD_DENY_EXTRA", "")
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


# ── One-time discovery caches ────────────────────────────────────────────────
# These are resolved once per process and reused across every run_command call.
# Discovery is pure filesystem probing — no mutable state, so a lock guards the
# (idempotent) first write and reads are unlocked afterwards.

_BASH_PATH_CACHE: Optional[str] = None
_BASH_PATH_SOURCE: Optional[str] = None  # env override that produced the cache
_VENV_CACHE: Optional[str] = None
_VENV_KEY_CACHE: Optional[str] = None
_DISCOVERY_LOCK = threading.Lock()

# Base environment: os.environ snapshot + GH_TOKEN/.env resolution + PATH prepend.
# Built once; per-call env = copy of this + workdir-independent tweaks.
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
    # The harness root is this repo (tools/MainTool/runCommand.py -> 3 dirnames
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
    GH_TOKEN from .env (lazy) + venv PATH prepend. Per-call copies are cheap."""
    global _BASE_ENV_CACHE, _ENV_SOURCE_HASH
    if _BASE_ENV_CACHE is not None and _ENV_SOURCE_HASH == id(os.environ):
        return _BASE_ENV_CACHE
    with _DISCOVERY_LOCK:
        if _BASE_ENV_CACHE is not None and _ENV_SOURCE_HASH == id(os.environ):
            return _BASE_ENV_CACHE
        env = dict(os.environ)
        # gh CLI reads GH_TOKEN (not GITHUB_TOKEN): expose it so run_command("gh ...")
        # composes without the typed github tools. .env is parsed ONCE here.
        if not env.get("GH_TOKEN"):
            gh_alt = env.get("GITHUB_TOKEN")
            if not gh_alt:
                try:
                    from dotenv import load_dotenv
                    load_dotenv()
                    gh_alt = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
                except Exception:
                    pass
            if gh_alt:
                env["GH_TOKEN"] = gh_alt
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


def _bash_env_file() -> Optional[str]:
    """Create (once) the persistent BASH_ENV file. Every `bash -lc` inherits
    these via BASH_ENV, so the model's shell functions/aliases/hygiene vars
    persist across calls without per-call setup.

    PATH is intentionally NOT exported here: the real PATH already reaches bash
    through the Popen env (custom_env), and exporting a Windows `;`-separated
    PATH inside BASH_ENV breaks Git Bash's /etc/profile setup (mtab/hosts
    links). BASH_ENV carries only vars that are safe to source early.
    Disable with AGENT_BASH_ENV=0."""
    global _BASH_ENV_FILE
    if _BASH_ENV_FILE is not None:
        return _BASH_ENV_FILE
    if os.environ.get("AGENT_BASH_ENV", "1").lower() in ("0", "false", "no"):
        return None
    env = _base_env()
    lines = []
    for k in ("GH_TOKEN", "GITHUB_TOKEN"):
        v = env.get(k)
        if v is None:
            continue
        v = str(v).replace("\\", "/").replace("'", "'\\''")
        lines.append(f"export {k}='{v}'")
    lines.append("export PYTHONUNBUFFERED=1")
    lines.append("export PIP_DISABLE_PIP_VERSION_CHECK=1")
    lines.append("export PYTHONDONTWRITEBYTECODE=1")
    lines.append("export LC_ALL=C.UTF-8 2>/dev/null || true")
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


def _per_call_env() -> dict:
    """Cheap per-call environment: one dict copy of the cached base."""
    return dict(_base_env())


class LocalShellBackend:
    def __init__(self, root_dir: str):
        self.root_dir = root_dir

    def execute(self, command: str, timeout: int = 30000, background: bool = False,
                name: str = None, shell: str = "bash", workdir: str = "", expect: str = "", stream: bool = False) -> str:
        global BACKGROUND_PROCESSES
        import os
        import re
        import subprocess
        import threading
        import queue
        import time
        from tools import _HARNESS_ROOT, auto_offload_result, _tool_cap, _RUN_OUTPUT_CACHE

        # Workdir override
        effective_dir = self.root_dir
        if workdir and str(workdir).strip():
            wd = str(workdir).strip()
            # Allow absolute or relative to root_dir
            if os.path.isabs(wd):
                effective_dir = os.path.normpath(wd)
            else:
                effective_dir = os.path.normpath(os.path.join(self.root_dir, wd))
            if not os.path.isdir(effective_dir):
                return f"Error: workdir '{workdir}' does not exist or is not a directory."

        # Per-call environment: one dict copy of the cached base (PATH venv-prepend,
        # GH_TOKEN, PYTHONUNBUFFERED...). Discovery happened once at module load.
        custom_env = _per_call_env()

        # Bash-only: harness uses Git Bash 5.3.15 (ships with Git 2.55.0). No cmd/powershell fallback.
        if shell != "bash":
            return f"Error: only shell='bash' is supported (got '{shell}'). All run_command calls must use bash - Git Bash 5.3.15 via _find_bash() :88-95."
        bash = _find_bash()
        if not bash:
            return "Error: bash not found — Git for Windows 2.55+ (bash 5.3.15) required at C:\\Program Files\\Git\\bin\\bash.exe; install from https://git-scm.com/download/win"
        use_bash = True

        # Persistent env file: BASH_ENV exports PATH/tokens/hygiene vars into every
        # bash -lc. Commands therefore inherit the same environment and can define
        # shell functions/aliases that persist across calls (Claude Code/Cursor-style).
        bash_env = _bash_env_file()
        if bash_env:
            custom_env["BASH_ENV"] = bash_env

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

        popen_kwargs = {"args": [bash, "-lc", command], "shell": False}

        if background:
            try:
                proc = subprocess.Popen(
                    **popen_kwargs,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    cwd=effective_dir,
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
                            from sync.state_sync import safe_append_live_log
                            # `line` already carries its trailing newline
                            safe_append_live_log(f"[{name}] {line}", newline=False)
                        except Exception:
                            pass
                            
                t = threading.Thread(target=_log_reader, daemon=True)
                t.start()
                info["thread"] = t
                with _BACKGROUND_PROCS_LOCK:
                    BACKGROUND_PROCESSES[name] = info
                
                # Quick liveness probe: 0.2s instead of a 1.5s fixed sleep. A slow
                # compile/install may legitimately produce nothing in that window,
                # so only an ACTUAL exit (not silence) is treated as a start failure.
                time.sleep(0.2)
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
                    **popen_kwargs,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    cwd=effective_dir,
                    env=custom_env,
                    encoding="utf-8",
                    errors="replace",
                )
                
                # Chunked live-log flush: the reader thread drains stdout into a local
                # buffer; every ~200ms the main loop flushes the accumulated lines to
                # the live log in ONE append (one state_version bump + one string copy
                # per chunk instead of per line — the SSE streamer polls state_version,
                # so batching is invisible to the frontend).
                q = queue.Queue()
                out_lines = []
                pending = []
                _FLUSH_INTERVAL = 0.2
                
                def read_output(stream, q):
                    for line in stream:
                        q.put(line)
                    stream.close()
                    
                t = threading.Thread(target=read_output, args=(proc.stdout, q))
                t.daemon = True
                t.start()
                
                start_time = time.time()
                max_duration = timeout / 1000.0
                
                def _flush_pending() -> None:
                    if pending:
                        safe_append_live_log("".join(pending), newline=False)
                        pending.clear()
                
                while True:
                    drained = 0
                    while not q.empty():
                        try:
                            line = q.get_nowait()
                            out_lines.append(line)
                            pending.append(line)
                            drained += 1
                        except queue.Empty:
                            break
                    if drained:
                        _flush_pending()
                    
                    if proc.poll() is not None:
                        t.join(timeout=1.0)
                        while not q.empty():
                            try:
                                line = q.get_nowait()
                                out_lines.append(line)
                                pending.append(line)
                            except queue.Empty:
                                break
                        _flush_pending()
                        break
                    
                    if time.time() - start_time > max_duration:
                        proc.kill()
                        t.join(timeout=1.0)
                        while not q.empty():
                            try:
                                line = q.get_nowait()
                                out_lines.append(line)
                                pending.append(line)
                            except queue.Empty:
                                break
                        _flush_pending()
                        output = "".join(out_lines)
                        cleaned = _clean_terminal_output(output)
                        capped = auto_offload_result(cleaned, command, max_chars=_tool_cap("run_command_timeout"))
                        result_str = f"Error: Command timed out after {timeout}ms: {command}\nPartial Output:\n{capped}\n[TIMEOUT]"
                        safe_append_live_log("\n[TIMEOUT]")
                        _log_tool("run_command_timeout", {"command": command}, result_str[:500])
                        return result_str
                    
                    time.sleep(_FLUSH_INTERVAL)
                    
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
                # Expect regex check
                if expect and str(expect).strip():
                    try:
                        import re as _re
                        pat = _re.compile(str(expect).strip())
                        if pat.search(result_str):
                            result_str += f"\n[EXPECT matched: {expect}]"
                        else:
                            result_str += f"\n[EXPECT failed: pattern '{expect}' not found]"
                    except Exception as e:
                        result_str += f"\n[EXPECT error: {e}]"

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


def run_command(ctx: object = None, command: str = "", timeout: int = 30000, background: bool = False,
                process_action: str = "", shell: str = "bash", workdir: str = "", expect: str = "", stream: bool = False) -> str:
    """Execute a shell command, manage background processes, or view logs.

    shell: 'bash' only — Git Bash 5.3.15 (ships with Git 2.55.0). Pipelines use POSIX semantics (pipes, &&, $(...), jq, gh).
    Passing shell='cmd' now errors; migrate to bash -lc syntax.
    workdir: working directory to run in.
    expect: regex to check in output — adds [EXPECT matched/failed].
    stream: if true, hints to stream output (currently returns full output).
    ``ctx`` is the optional typed WorkspaceDeps; legacy callers omit it.
    """
    global BACKGROUND_PROCESSES
    import threading
    import re
    from tools import _FILE_CACHE, _increment_cache_version, shared_state, WORKSPACE, auto_offload_result, _tool_cap

    # If timeout is passed in seconds instead of milliseconds, scale it
    if timeout is not None and timeout < 1000:
        timeout = timeout * 1000

    # Clear local file cache since executing a command can modify files on disk
    _FILE_CACHE.clear()
    _increment_cache_version()

    active_ws = shared_state.get("project_path") or WORKSPACE
    if ctx is not None:
        try:
            active_ws = getattr(ctx, "project_path", None) or active_ws
        except Exception:
            pass
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
    result = backend.execute(command, timeout=timeout, background=background, name=name, shell=shell, workdir=workdir, expect=expect, stream=stream)
    if isinstance(result, str):
        return _clean_terminal_output(result)
    return result
