"""MainTool.runCommand — the run_command tool and its shell backend.

Shell policy: Git Bash only (POSIX semantics: pipes, &&, $( ), jq, gh). Windows
tools remain reachable as subprocesses: `powershell.exe -NoProfile -Command
"..."` or `cmd.exe /c "..."`.

Modules:
  * `_shellenv`  — bash discovery, cached base env, BASH_ENV, scoped credentials.
  * `_cmdpolicy` — command parsing + destructive/read-only/server-like policy.
  * `_procman`   — process-tree kill, background registry, spill logs, lifecycle.

Execution model:
  * Foreground blocks, streams output to the live log in ~80ms chunks, and is
    bounded by a wall timeout (default CMD_DEFAULT_TIMEOUT_MS=120000) AND an
    idle timeout (default CMD_IDLE_TIMEOUT_MS=60000; 0 disables).
  * Timeouts kill the whole process tree (psutil TERM -> grace -> KILL) so no
    grandchild server survives; readers drain with a deadline afterwards.
  * Server-like commands (npm run dev, vite, uvicorn, python -m http.server,
    ...) that hit a timeout are moved to the background instead of killed.
  * Background processes get a spill-to-disk log, exit codes, and lifecycle
    actions (list/logs/kill/wait/kill_all), with owner-scoped cleanup.
"""
import hashlib
import os
import re
import subprocess
import time
from typing import Any

from sync.state_sync import safe_append_live_log

from ._cmdpolicy import (
    BLOCKED_PATTERNS,
    check_command,
    expected_nonzero,
    is_read_only,
    is_server_like,
)
from ._procman import (
    IO_DRAIN_TIMEOUT,
    OutputPump,
    generate_name,
    kill as proc_kill,
    kill_all,
    kill_tree,
    list_processes,
    logs as proc_logs,
    register,
    resolve_owner,
    scan_ready,
    send as proc_send,
    tree_cpu_seconds,
    wait as proc_wait,
    wait_ready,
)
from ._shared import _clean_terminal_output, _log_tool
from ._shellenv import _bash_env_file, _find_bash, bash_args, per_call_env

__all__ = [
    "run_command",
    "LocalShellBackend",
    "BLOCKED_PATTERNS",
    "_find_bash",
    "_bash_env_file",
    "bash_args",
    "per_call_env",
]

_TIMEOUT_MAX = 600000
_UNSET = -1
_REPEAT_MIN_RUN = 5

_NONPRINT_RE = re.compile(r"[^\x09\x0a\x0d\x20-\x7e\u0080-\uffff]")


def _command_target(decision: Any) -> str:
    """A path-free, stable shape target for one command: its program roots.

    ``gh pr create --body "D:\\project\\x"`` must not put an absolute path in
    an approval shape or an idempotency key (``06`` §7). The roots are the
    program basenames ``check_command`` already parsed; the full command is
    hashed separately as the payload identity.
    """
    roots = [str(root) for root in (getattr(decision, "roots", None) or [])
             if root]
    return "+".join(sorted(set(roots))) or "shell"


def _command_succeeded(result: Any) -> bool:
    """Only ``[OK]`` means the shell completed a successful attempt.

    ``[ERR]``, ``[TIMEOUT]`` and ``Error:`` leave the AU-05 reservation in
    place, so a retry is parked as "may or may not have been sent" rather than
    silently skipping a command that failed.
    """
    return isinstance(result, str) and result.lstrip().startswith("[OK]")


def _env_int(name: str, default: int) -> int:
    try:
        return int(os.environ.get(name, str(default)))
    except Exception:
        return default


def _default_wall_ms() -> int:
    return max(1000, _env_int("CMD_DEFAULT_TIMEOUT_MS", 120000))


def _default_idle_ms() -> int:
    return max(0, _env_int("CMD_IDLE_TIMEOUT_MS", 60000))


def _resolve_wall(timeout) -> int:
    if timeout is None or timeout == _UNSET:
        return _default_wall_ms()
    try:
        t = int(timeout)
    except Exception:
        return _default_wall_ms()
    if t <= 0:
        return _default_wall_ms()
    if t < 1000:  # legacy: values under a second were seconds
        t *= 1000
    return min(t, _TIMEOUT_MAX)


def _resolve_idle(idle_timeout) -> int:
    """0 disables; unset uses the env default; <1000 treated as seconds."""
    if idle_timeout is None or idle_timeout == _UNSET:
        return _default_idle_ms()
    try:
        i = int(idle_timeout)
    except Exception:
        return _default_idle_ms()
    if i == 0:
        return 0
    if i < 0:
        return _default_idle_ms()
    if i < 1000:
        i *= 1000
    return min(i, _TIMEOUT_MAX)


def _cpu_min_step() -> float:
    """Fraction of one CPU core over the sample window treated as activity."""
    try:
        return max(0.0, float(os.environ.get("CMD_IDLE_CPU_MIN", "0.10")))
    except Exception:
        return 0.10


def _bg_activity_window_s() -> float:
    """Recent-activity window for auto-backgrounding on wall timeout."""
    try:
        return max(0.0, float(os.environ.get("CMD_BG_ACTIVITY_WINDOW_MS", "10000")) / 1000.0)
    except Exception:
        return 10.0


def _ready_default_ms() -> int:
    return max(0, _env_int("CMD_READY_TIMEOUT_MS", 15000))


def _group_flags() -> dict:
    """Spawn in its own process group/session so the whole tree is killable."""
    if os.name == "nt":
        return {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
    return {"start_new_session": True}


def _looks_binary(raw: str) -> bool:
    if not raw:
        return False
    if "\x00" in raw:
        return True
    sample = raw[:8000]
    bad = len(_NONPRINT_RE.findall(sample))
    return len(sample) > 0 and (bad / len(sample)) > 0.25


def _compress_enabled() -> bool:
    return os.environ.get("CMD_COMPRESS_REPEATS", "1").lower() not in ("0", "false", "no")


def _compress_repeats(text: str, min_run: int = _REPEAT_MIN_RUN):
    """Collapse runs of identical consecutive lines. Returns (text, changed)."""
    if not text:
        return text, False
    lines = text.split("\n")
    out = []
    changed = False
    i = 0
    while i < len(lines):
        j = i
        while j + 1 < len(lines) and lines[j + 1] == lines[i]:
            j += 1
        run = j - i + 1
        if run >= min_run and lines[i].strip():
            out.append(lines[i])
            out.append(f"  ... [repeated x{run - 1} — identical lines collapsed]")
            changed = True
        else:
            out.extend(lines[i:j + 1])
        i = j + 1
    return "\n".join(out), changed


class LocalShellBackend:
    def __init__(self, root_dir: str):
        self.root_dir = root_dir

    def execute(self, command: str, timeout=None, background: bool = False,
                name: str = None, shell: str = "bash", workdir: str = "",
                expect: str = "", idle_timeout=None, owner: str = "",
                ready_timeout: int = 15000) -> str:
        from tools import auto_offload_result, _tool_cap, _RUN_OUTPUT_CACHE

        wall_ms = _resolve_wall(timeout)
        idle_ms = _resolve_idle(idle_timeout)

        # Workdir override
        effective_dir = self.root_dir
        if workdir and str(workdir).strip():
            wd = str(workdir).strip()
            if os.path.isabs(wd):
                effective_dir = os.path.normpath(wd)
            else:
                effective_dir = os.path.normpath(os.path.join(self.root_dir, wd))
            if not os.path.isdir(effective_dir):
                from pydantic_ai.exceptions import ToolFailed
                raise ToolFailed(f"Error: workdir '{workdir}' does not exist or is not a directory.")

        # Credentials are scoped to commands that authenticate (gh/git/curl/...).
        custom_env = per_call_env(command)

        if shell != "bash":
            from pydantic_ai.exceptions import ToolFailed
            raise ToolFailed(
                f"Error: only shell='bash' is supported (got '{shell}'). Git Bash is the single "
                "shell; run Windows tools as subprocesses instead, e.g. "
                "powershell.exe -NoProfile -Command \"Get-Content file\" or cmd.exe /c \"dir\"."
            )
        bash = _find_bash()
        if not bash:
            from pydantic_ai.exceptions import ToolFailed
            raise ToolFailed("Error: bash not found — Git for Windows 2.55+ (bash 5.3.15) required at C:\\Program Files\\Git\\bin\\bash.exe; install from https://git-scm.com/download/win")

        bash_env = _bash_env_file()
        if bash_env:
            custom_env["BASH_ENV"] = bash_env

        docker_name = ""
        use_docker = os.environ.get("DEEP_AGENTS_USE_DOCKER", "false").lower() == "true"
        if use_docker:
            abs_root = os.path.abspath(self.root_dir)
            image = os.environ.get("DEEP_AGENTS_DOCKER_IMAGE", "python:3.10-slim")
            if background and name:
                docker_name = f"ai_run_{re.sub(r'[^A-Za-z0-9_.-]', '_', name)}"
            name_flag = f"--name {docker_name} " if docker_name else ""
            escaped_command = command.replace('\\', '\\\\')
            escaped_command = escaped_command.replace('`', '\\`')
            escaped_command = escaped_command.replace('$', '\\$')
            escaped_command = escaped_command.replace('"', '\\"')
            command = f'docker run --rm {name_flag}-v "{abs_root}:/workspace" -w /workspace {image} sh -c "{escaped_command}"'

        popen_kwargs = {
            "args": [bash, *bash_args(), command],
            "shell": False,
            "stdout": subprocess.PIPE,
            "stderr": subprocess.STDOUT,
            "text": True,
            "cwd": effective_dir,
            "env": custom_env,
            "encoding": "utf-8",
            "errors": "replace",
            **_group_flags(),
        }

        if background:
            popen_kwargs["stdin"] = subprocess.PIPE
            return self._run_background(command, name, popen_kwargs, owner, docker_name,
                                        expect=expect, ready_timeout=ready_timeout)
        return self._run_foreground(command, wall_ms, idle_ms, expect, effective_dir, owner,
                                    docker_name, popen_kwargs, auto_offload_result, _tool_cap,
                                    _RUN_OUTPUT_CACHE)

    def _run_background(self, command, name, popen_kwargs, owner, docker_name,
                        expect: str = "", ready_timeout: int = 15000) -> str:
        try:
            proc = subprocess.Popen(**popen_kwargs)
        except Exception as e:
            return f"Error starting background process: {e}"
        pump = OutputPump(proc)
        register(name, proc, pump, command, popen_kwargs.get("cwd", ""), owner=owner,
                 extra={"docker_name": docker_name})
        # 0.2s liveness probe: only an actual exit means the start failed.
        time.sleep(0.2)
        rc = proc.poll()
        if rc is not None:
            pump.thread.join(timeout=1.0)
            from ._procman import reap_finished, logs as _logs
            reap_finished()
            output = _logs(name, tail=100)
            _log_tool("run_command_bg_fail", {"command": command, "name": name}, output[:500])
            return f"[ERR] Background process '{name}' failed to start immediately (exit {rc}).\n{output}"

        # Readiness: wait for expect / a listening port for server-like commands.
        expect_re = None
        if expect and str(expect).strip():
            try:
                expect_re = re.compile(str(expect).strip())
            except Exception:
                expect_re = None
        info = None
        if expect_re or is_server_like(command):
            info = wait_ready(name, expect_re, ready_timeout)
        else:
            info = scan_ready(name, None)
        rc = proc.poll()
        if rc is not None:
            pump.thread.join(timeout=1.0)
            from ._procman import reap_finished, logs as _logs
            reap_finished()
            output = _logs(name, tail=100)
            _log_tool("run_command_bg_fail", {"command": command, "name": name}, output[:500])
            return f"[ERR] Background process '{name}' exited early (exit {rc}).\n{output}"

        ready_note = ""
        if info and info.get("url"):
            ready_note = f"\n[READY] listening on {info['url']}"
        elif expect_re and info and info.get("expect"):
            ready_note = f"\n[READY] expect matched: {expect}"
        elif expect_re:
            ready_note = (f"\n[READY] expect '{expect}' not seen after {int(ready_timeout)}ms "
                          f"(still starting? check logs)")
        output = (
            f"[OK] Started background process '{name}' (PID {proc.pid}).{ready_note}\n"
            f"Use process_action='logs' command='{name}' to view output; "
            f"'ready' to re-check readiness; 'wait' to block on it; "
            f"'send' to write to stdin; 'kill' to stop it."
        )
        _log_tool("run_command_bg", {"command": command, "name": name}, output)
        return output

    def _run_foreground(self, command, wall_ms, idle_ms, expect, effective_dir, owner,
                        docker_name, popen_kwargs, auto_offload_result, _tool_cap,
                        run_output_cache) -> str:
        safe_append_live_log(f"\n[Terminal] Run: {command}")
        try:
            proc = subprocess.Popen(**popen_kwargs)
        except Exception as e:
            return f"Error running command: {e}"

        pump = OutputPump(proc)
        start = time.time()
        flush_interval = 0.08
        wall_hit = idle_hit = False
        elapsed_ms = idle_for_ms = 0.0
        # CPU-activity tracking: a silent-but-busy build (tsc/webpack/docker
        # build) is working, not hung, so it must not trip the idle timeout.
        cpu_last_sample = start
        cpu_last_total = tree_cpu_seconds(proc)
        last_cpu_activity = start

        while True:
            pending = pump.drain_pending()
            if pending:
                safe_append_live_log("".join(pending), newline=False)

            if proc.poll() is not None:
                break

            now = time.time()
            if now - cpu_last_sample >= 0.5:
                total = tree_cpu_seconds(proc)
                if total is not None and cpu_last_total is not None:
                    delta = total - cpu_last_total
                    if delta >= _cpu_min_step() * (now - cpu_last_sample):
                        last_cpu_activity = now
                cpu_last_total = total
                cpu_last_sample = now

            elapsed_ms = (now - start) * 1000.0
            last_activity = max(pump.last_activity, last_cpu_activity)
            idle_for_ms = (now - last_activity) * 1000.0
            wall_hit = bool(wall_ms) and elapsed_ms > wall_ms
            idle_hit = bool(idle_ms) and idle_for_ms > idle_ms
            if wall_hit or idle_hit:
                reason = (f"no output for {int(idle_for_ms)}ms" if (idle_hit and not wall_hit)
                          else f"wall limit {wall_ms}ms")
                # CPU activity already feeds ``last_activity``, so an idle hit
                # means genuinely idle: only servers get adopted there. On a
                # wall hit, an actively-working command is adopted too.
                cpu_recent = (now - last_cpu_activity) <= max(1.0, _bg_activity_window_s())
                adopt_cpu = bool(wall_hit and not idle_hit and cpu_recent)
                if is_server_like(command) or adopt_cpu:
                    pname = generate_name(command)
                    register(pname, proc, pump, command, effective_dir, owner=owner,
                             extra={"docker_name": docker_name})
                    pending = pump.drain_pending()
                    if pending:
                        safe_append_live_log("".join(pending), newline=False)
                    why = f"{reason}; CPU active" if adopt_cpu else reason
                    output = (
                        f"[OK] '{pname}' still running after {int(elapsed_ms)}ms ({why}) — "
                        f"moved to background (PID {proc.pid}).\n"
                        f"Use process_action='logs' command='{pname}' to view output; "
                        f"'kill' to stop it."
                    )
                    _log_tool("run_command_bg", {"command": command, "name": pname}, output[:500])
                    return output
                killed = kill_tree(proc)
                pump.thread.join(timeout=IO_DRAIN_TIMEOUT)
                output = "".join(pump.lines)
                cleaned = _clean_terminal_output(output) or "(no output)"
                capped = auto_offload_result(cleaned, command,
                                             max_chars=_tool_cap("run_command_timeout"))
                detail = (f"no output for {int(idle_for_ms)}ms (idle limit {idle_ms}ms)"
                          if (idle_hit and not wall_hit) else f"wall limit {wall_ms}ms")
                result_str = (
                    f"Error: Command timed out after {int(elapsed_ms)}ms — {detail} "
                    f"[exit 124]; killed {killed} process(es): {command}\n"
                    f"Partial output:\n{capped}\n[TIMEOUT]"
                )
                safe_append_live_log("\n[TIMEOUT]")
                _log_tool("run_command_timeout", {"command": command}, result_str[:500])
                return result_str

            time.sleep(flush_interval)

        pump.thread.join(timeout=IO_DRAIN_TIMEOUT)
        pending = pump.drain_pending()
        if pending:
            safe_append_live_log("".join(pending), newline=False)
        try:
            exit_code = proc.wait()
        except Exception:
            exit_code = proc.poll()
        duration_ms = int((time.time() - start) * 1000.0)
        raw_output = "".join(pump.lines)
        cleaned = _clean_terminal_output(raw_output)
        # grep/find/diff exit 1 is an answer ("no match"/"differences"), not a failure.
        negative_label = expected_nonzero(exit_code, command) if exit_code != 0 else ""
        if negative_label:
            header = f"[OK] Exit {exit_code} ({negative_label}, {duration_ms}ms)"
        else:
            status = "OK" if exit_code == 0 else "ERR"
            header = f"[{status}] Exit {exit_code} ({duration_ms}ms)"

        if _looks_binary(raw_output):
            size = len(raw_output.encode("utf-8", errors="replace"))
            result_str = (
                f"{header}\n[binary output: {size} bytes not shown — "
                f"redirect it to a file (e.g. `cmd > out.bin`) and inspect that]"
            )
        else:
            capped = auto_offload_result(cleaned, command, max_chars=_tool_cap("run_command"))
            compressed = False
            if capped == cleaned and _compress_enabled():
                capped, compressed = _compress_repeats(capped)
            if capped:
                result_str = f"{header}\n{capped}"
                if compressed:
                    result_str += "\n[... repeated output compressed — set CMD_COMPRESS_REPEATS=0 to disable]"
            else:
                result_str = header

            if expect and str(expect).strip():
                try:
                    pat = re.compile(str(expect).strip())
                    if pat.search(cleaned):
                        result_str += f"\n[EXPECT matched: {expect}]"
                    else:
                        result_str += f"\n[EXPECT failed: pattern '{expect}' not found]"
                except Exception as e:
                    result_str += f"\n[EXPECT error: {e}]"

        # Run-output dedup: re-running the same command in the same workdir with
        # identical output returns a stub instead of re-appending the text.
        # Hash is duration-independent (exit code + cleaned output + expect).
        try:
            run_key = f"{effective_dir}\x00{command.strip()}"
            stable = f"{exit_code}\n{cleaned}\n{expect or ''}"
            run_hash = hashlib.md5(stable.encode("utf-8", errors="replace")).hexdigest()
            if run_key in run_output_cache and run_output_cache[run_key] == run_hash:
                result_str = (
                    f"{header}\n[REPEAT RUN — identical output to an earlier run of this "
                    f"command in this directory; the full output is already in this conversation.]"
                )
            else:
                run_output_cache[run_key] = run_hash
        except Exception:
            pass

        _log_tool("run_command", {"command": command}, result_str[:500])
        return result_str


def run_command(ctx: object = None, command: str = "", timeout=_UNSET, background: bool = False,
                process_action: str = "", shell: str = "bash", workdir: str = "",
                expect: str = "", idle_timeout=_UNSET, tail_lines: int = 200,
                ready_timeout: int = 15000, input: str = "") -> str:
    """Execute a shell command, manage background processes, or view logs.

    Shell: Git Bash only. Run Windows tools as subprocesses, e.g.
    ``powershell.exe -NoProfile -Command "..."`` or ``cmd.exe /c "..."``.

    timeout: wall limit in ms (default CMD_DEFAULT_TIMEOUT_MS=120000, max 600000;
        values <1000 are treated as seconds). Use >=60000 for installs.
    idle_timeout: kill after this many ms without output or CPU activity
        (default CMD_IDLE_TIMEOUT_MS=60000; 0 disables).
    background: start detached; the returned name drives process_action.
    process_action: 'list' | 'logs' | 'kill' | 'wait' | 'ready' | 'send' |
        'kill_all' (command carries the process name for logs/kill/wait/ready/send).
    workdir: working directory (default project root).
    expect: regex checked against the full output; adds [EXPECT matched/failed].
        For background starts it is also awaited up to ready_timeout.
    tail_lines: lines returned by process_action='logs' (default 200).
    ready_timeout: background readiness wait in ms (default 15000).
    input: text written to the process stdin by process_action='send'.
    ``ctx`` is the optional typed WorkspaceDeps; legacy callers omit it.
    """
    from tools import _FILE_CACHE, _increment_cache_version, shared_state, WORKSPACE

    # Invalidate file/tool caches only for destructive shells — read-only
    # shells must not invalidate parallel sibling reads.
    _should_clear = True
    try:
        from tools.plan_mode_guard import _is_destructive_shell as _is_dest
        _should_clear = _is_dest(str(command or ""))
    except Exception:
        _should_clear = True
    if _should_clear and not process_action:
        _FILE_CACHE.clear()
        _increment_cache_version()

    active_ws = shared_state.get("project_path") or WORKSPACE
    if ctx is not None:
        try:
            active_ws = getattr(ctx, "project_path", None) or active_ws
        except Exception:
            pass
    active_ws = os.path.normpath(os.path.abspath(active_ws))
    owner = resolve_owner(ctx)

    # Background lifecycle actions (``command`` carries the process name).
    if process_action:
        action = str(process_action).strip().lower()
        pname = str(command or "").strip()
        if action == "list":
            return list_processes()
        if action == "logs":
            if not pname:
                return "Error: process_action='logs' requires command='<process name>'."
            return proc_logs(pname, tail=tail_lines)
        if action == "kill":
            if not pname:
                return "Error: process_action='kill' requires command='<process name>'."
            return proc_kill(pname)
        if action == "wait":
            if not pname:
                return "Error: process_action='wait' requires command='<process name>'."
            return proc_wait(pname, timeout_ms=_resolve_wall(timeout))
        if action == "ready":
            if not pname:
                return "Error: process_action='ready' requires command='<process name>'."
            expect_re = None
            if expect and str(expect).strip():
                try:
                    expect_re = re.compile(str(expect).strip())
                except Exception:
                    expect_re = None
            info = scan_ready(pname, expect_re)
            if info is None:
                return f"Error: No background process named '{pname}' (running or recent)."
            bits = [f"status={info.get('status')}"]
            if info.get("url"):
                bits.append(f"listening on {info['url']}")
            if expect_re:
                bits.append(f"expect={'matched' if info.get('expect') else 'not seen'}")
            return "[READY] " + ", ".join(bits)
        if action == "send":
            if not pname:
                return "Error: process_action='send' requires command='<process name>'."
            return proc_send(pname, input)
        if action == "kill_all":
            n = kill_all()
            return f"[OK] Killed {n} background process(es)." if n else "No background processes to kill."
        return (f"Error: Invalid process_action '{process_action}' "
                f"(use list|logs|kill|wait|ready|send|kill_all).")

    if not command:
        from pydantic_ai.exceptions import ToolFailed
        raise ToolFailed("Error: command parameter is required.")

    # Safety validation (token-aware policy + env-extensible deny list).
    decision = check_command(command)
    if not decision.allowed:
        from pydantic_ai.exceptions import ToolFailed
        raise ToolFailed(
            f"Error: Command execution blocked for safety — {decision.reason or decision.rule}."
        )

    name = None
    if background:
        name = generate_name(command)

    backend = LocalShellBackend(root_dir=active_ws)

    def execute() -> Any:
        return backend.execute(
            command, timeout=timeout, background=background, name=name,
            shell=shell, workdir=workdir, expect=expect,
            idle_timeout=idle_timeout, owner=owner,
            ready_timeout=max(0, int(ready_timeout or 0)),
        )

    # Read-only commands cannot have an outward effect, so they do not need
    # the envelope or the ledger. Everything else asks the gate (which parks
    # the item on `needs_approval`) and then runs through AU-05's ledger.
    read_only, _why = is_read_only(command)
    if read_only:
        result = execute()
    else:
        from tools._runtime import (
            TIER_AUTONOMOUS,
            TIER_NEEDS_APPROVAL,
            WRITE_SKIPPED,
            ApprovalRequired,
            _authorize,
            _perform_write,
        )

        target = _command_target(decision)
        tier = _authorize("command", target)
        if tier == TIER_NEEDS_APPROVAL:
            # The gate has already parked the item; end the run rather than
            # turning an approval wait into an ordinary tool error.
            raise ApprovalRequired(
                f"command {command!r} is waiting for the owner")
        if tier not in (None, TIER_AUTONOMOUS):
            raise PermissionError(
                f"command {command!r} is refused by this project's autonomy "
                f"envelope")

        result = _perform_write(
            "command", target, command, execute,
            succeeded=_command_succeeded,
        )
        if result == WRITE_SKIPPED:
            return result

    if isinstance(result, str):
        return _clean_terminal_output(result)
    return result
