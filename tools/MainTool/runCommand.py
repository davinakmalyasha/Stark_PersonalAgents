"""MainTool.runCommand — the run_command tool and its shell backend.

Holds run_command, LocalShellBackend, the blocked-command policy
(BLOCKED_PATTERNS + env-extensible deny list), bash discovery, and the
multiline `python -c` rewrite. tools.py state helpers (_HARNESS_ROOT,
_FILE_CACHE, _increment_cache_version, shared_state, WORKSPACE,
auto_offload_result, _tool_cap, _RUN_OUTPUT_CACHE) are imported lazily
inside the functions to avoid an import cycle.
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


def _find_bash() -> Optional[str]:
    """Locate a POSIX bash for run_command. Prefers git-bash (ships with Git for
    Windows); rejects the WSL shim (System32\\bash.exe) which only works if WSL
    is installed."""
    env_bash = os.environ.get("BASH_PATH", "").strip()
    if env_bash and os.path.isfile(env_bash):
        return env_bash
    candidates = [
        os.path.join(os.environ.get("ProgramFiles", r"C:\Program Files"), "Git", "bin", "bash.exe"),
        os.path.join(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"), "Git", "bin", "bash.exe"),
        os.path.join(os.environ.get("LocalAppData", ""), "Programs", "Git", "bin", "bash.exe"),
    ]
    for c in candidates:
        if c and os.path.isfile(c):
            return c
    import shutil
    found = shutil.which("bash")
    if found:
        low = found.lower()
        if "system32" not in low and "syswow64" not in low:
            return found
    return None


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

        # Construct custom environment with prepended virtual environment PATH
        custom_env = os.environ.copy()
        # gh CLI reads GH_TOKEN (not GITHUB_TOKEN): expose it so run_command("gh ...")
        # composes without the typed github tools.
        if not custom_env.get("GH_TOKEN"):
            gh_alt = custom_env.get("GITHUB_TOKEN")
            if not gh_alt:
                try:
                    from dotenv import load_dotenv
                    load_dotenv()
                    gh_alt = os.environ.get("GITHUB_TOKEN")
                except Exception:
                    pass
            if gh_alt:
                custom_env["GH_TOKEN"] = gh_alt
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
        # (bash mode handles multiline -c natively, so only rewrite for cmd)
        use_bash = shell == "bash"
        bash = _find_bash() if use_bash else None
        if use_bash and bash is None:
            use_bash = False
            shell = "cmd"
        if not use_bash:
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

        popen_kwargs = {}
        if use_bash:
            popen_kwargs = {"args": [bash, "-lc", command], "shell": False}
        else:
            popen_kwargs = {"args": command, "shell": True}

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
                    **popen_kwargs,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    cwd=effective_dir,
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


def run_command(command: str = "", timeout: int = 30000, background: bool = False,
                process_action: str = "", shell: str = "bash", workdir: str = "", expect: str = "", stream: bool = False) -> str:
    """Execute a shell command, manage background processes, or view logs.

    shell: 'bash' (default — full POSIX via git-bash: pipes, &&, $(...), jq,
    gh pipelines) or 'cmd' (Windows command/batch compatibility).
    workdir: working directory to run in.
    expect: regex to check in output — adds [EXPECT matched/failed].
    stream: if true, hints to stream output (currently returns full output).
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
