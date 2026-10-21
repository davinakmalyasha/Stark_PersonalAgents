"""MainTool._procman — process spawning, process-tree kill, and the background registry.

Why this module exists:
  * Timeout/kill must take the whole process tree. Killing the direct ``bash``
    child used to leave ``node``/``vite``/``uvicorn`` grandchildren running and
    holding ports. psutil (already in the venv) gives us a portable recursive
    kill with a terminate -> grace -> kill escalation.
  * After a kill, grandchildren can still hold the inherited stdout/stderr
    pipes open, so readers must drain with a deadline instead of joining
    forever (the Codex ``IO_DRAIN_TIMEOUT_MS`` lesson).
  * Background processes get a spill-to-disk log (the old 2000-line deque lost
    early output), a status/exit code, and lifecycle actions (wait/logs/kill/
    kill_all), plus owner-scoped cleanup at run end.
"""
from __future__ import annotations

import atexit
import collections
import os
import re
import signal
import socket
import subprocess
import tempfile
import threading
import time

try:
    import psutil
except Exception:  # pragma: no cover - psutil is pinned in requirements.txt
    psutil = None

IO_DRAIN_TIMEOUT = 2.0
TERMINATE_GRACE = 2.0
_RECENT_MAX = 20
_TAIL_LINES = 4000
_TAIL_READ_BYTES = 262144

#: Ready-detection patterns for background servers (Vite/Next/uvicorn/etc).
_URL_RE = re.compile(
    r"https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])(?::(\d{2,5}))?",
    re.IGNORECASE,
)
_ADDR_PORT_RE = re.compile(
    r"(?:localhost|127\.0\.0\.1|0\.0\.0\.0)[\s:]+(\d{2,5})",
    re.IGNORECASE,
)

_PROCS: dict = {}
_RECENT: "collections.OrderedDict[str, dict]" = collections.OrderedDict()
_LOCK = threading.RLock()
_LOG_DIR = None


def _log_dir() -> str:
    global _LOG_DIR
    if _LOG_DIR is None:
        base = os.path.join(tempfile.gettempdir(), "ai_agent_run_logs")
        try:
            os.makedirs(base, exist_ok=True)
        except Exception:
            pass
        _LOG_DIR = base
    return _LOG_DIR


def _safe_name(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]", "_", str(name or "proc"))[:60]


def _tail_file(path: str, lines: int) -> str:
    """Read the last N lines of a log file without loading it entirely."""
    try:
        size = os.path.getsize(path)
        if size <= 0:
            return ""
        read = min(size, _TAIL_READ_BYTES)
        with open(path, "rb") as f:
            if size > read:
                f.seek(size - read)
            data = f.read().decode("utf-8", errors="replace")
        out = data.splitlines()
        if size > read and out:
            out = out[1:]  # first line may be partial
        return "\n".join(out[-lines:])
    except Exception:
        return ""


class OutputPump:
    """Reads a process's merged stdout once; feeds foreground and background.

    Foreground loops drain ``pending`` for the live log; when a long-running
    server outlives its wall/idle timeout the pump is adopted into the
    background registry and starts writing the spill log + prefixed live log.
    """

    def __init__(self, proc):
        self.proc = proc
        self.lines: list = []
        self.tail = collections.deque(maxlen=_TAIL_LINES)
        self.pending: list = []
        self.lock = threading.Lock()
        self.done = threading.Event()
        self.last_activity = time.time()
        self.adopted = False
        self.name = None
        self.log_path = None
        self._log_fh = None
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self):
        try:
            for line in self.proc.stdout:
                self.last_activity = time.time()
                self.lines.append(line)
                self.tail.append(line)
                if self.adopted:
                    self._write_line(line)
                    try:
                        from sync.state_sync import safe_append_live_log
                        safe_append_live_log(f"[{self.name}] {line}", newline=False)
                    except Exception:
                        pass
                else:
                    with self.lock:
                        self.pending.append(line)
        except Exception:
            pass
        finally:
            try:
                self.proc.stdout.close()
            except Exception:
                pass
            try:
                if self.proc.stdin is not None:
                    self.proc.stdin.close()
            except Exception:
                pass
            self._close_log()
            self.done.set()

    def _write_line(self, line: str) -> None:
        if self._log_fh is None:
            return
        try:
            self._log_fh.write(line)
            self._log_fh.flush()
        except Exception:
            pass

    def drain_pending(self) -> list:
        with self.lock:
            if not self.pending:
                return []
            out = self.pending
            self.pending = []
            return out

    def adopt(self, name: str, log_path: str) -> None:
        """Switch the pump to background mode; backfill the spill log."""
        self.adopted = True
        self.name = name
        self.log_path = log_path
        try:
            self._log_fh = open(log_path, "a", encoding="utf-8", errors="replace")
            for line in list(self.lines)[-_TAIL_LINES:]:
                self._log_fh.write(line)
            self._log_fh.flush()
        except Exception:
            self._log_fh = None

    def close(self) -> None:
        self._close_log()
        self.thread.join(timeout=IO_DRAIN_TIMEOUT)

    def _close_log(self) -> None:
        try:
            if self._log_fh is not None:
                self._log_fh.close()
        except Exception:
            pass
        self._log_fh = None


def kill_tree(proc) -> int:
    """Terminate the whole process tree: TERM -> grace -> KILL. Returns count."""
    if proc is None:
        return 0
    if psutil is not None:
        try:
            parent = psutil.Process(proc.pid)
            procs = parent.children(recursive=True)
            procs.append(parent)
        except Exception:
            procs = []
        if procs:
            for p in procs:
                try:
                    p.terminate()
                except Exception:
                    pass
            try:
                _gone, alive = psutil.wait_procs(procs, timeout=TERMINATE_GRACE)
            except Exception:
                alive = []
            for p in alive:
                try:
                    p.kill()
                except Exception:
                    pass
            try:
                proc.kill()
            except Exception:
                pass
            return len(procs)
    # psutil missing or the process was already gone: platform fallbacks.
    killed = 0
    try:
        if os.name == "nt":
            subprocess.run(
                ["taskkill", "/T", "/F", "/PID", str(proc.pid)],
                capture_output=True, timeout=5,
            )
        else:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        killed = 1
    except Exception:
        pass
    try:
        proc.kill()
    except Exception:
        pass
    return killed


def tree_cpu_seconds(proc):
    """Cumulative user+system CPU seconds of the whole process tree, or None."""
    if psutil is None or proc is None:
        return None
    try:
        parent = psutil.Process(proc.pid)
        procs = [parent] + parent.children(recursive=True)
    except Exception:
        return None
    total = 0.0
    for p in procs:
        try:
            t = p.cpu_times()
            total += float(t.user) + float(t.system)
        except Exception:
            pass
    return total


def generate_name(command: str) -> str:
    first = ""
    try:
        first = str(command or "").strip().split()[0]
    except Exception:
        first = ""
    base = re.sub(r"[^a-zA-Z0-9_-]", "_", first) or "proc"
    with _LOCK:
        name = base
        suffix = 1
        while name in _PROCS:
            name = f"{base}_{suffix}"
            suffix += 1
    return name


def register(name: str, proc, pump: OutputPump, command: str, cwd: str, owner: str = "",
             extra: dict = None) -> dict:
    """Adopt a process + pump into the background registry with a spill log."""
    log_path = os.path.join(_log_dir(), f"{_safe_name(name)}_{proc.pid}_{int(time.time())}.log")
    pump.adopt(name, log_path)
    entry = {
        "name": name,
        "proc": proc,
        "pump": pump,
        "command": command,
        "cwd": cwd,
        "owner": owner or "",
        "log_path": log_path,
        "started_at": time.time(),
        "finished_at": None,
        "exit_code": None,
        "status": "running",
        "killed": False,
    }
    if extra:
        entry.update(extra)
    with _LOCK:
        # A reused name must not leave a stale _RECENT entry that would make
        # kill()/send() think the live process already finished.
        _RECENT.pop(name, None)
        _PROCS[name] = entry
    return entry


def _lookup(name: str):
    with _LOCK:
        entry = _PROCS.get(name)
        if entry is not None:
            return entry
        return _RECENT.get(name)


def _move_to_recent(entry: dict, status: str, exit_code=None) -> None:
    entry["status"] = status
    if exit_code is not None:
        entry["exit_code"] = exit_code
    entry["finished_at"] = time.time()
    evicted = []
    with _LOCK:
        _PROCS.pop(entry["name"], None)
        _RECENT[entry["name"]] = entry
        while len(_RECENT) > _RECENT_MAX:
            _old_name, old = _RECENT.popitem(last=False)
            evicted.append(old)
    # Close/remove outside the lock: pump.close() joins the reader thread.
    for old in evicted:
        try:
            old["pump"].close()
        except Exception:
            pass
        try:
            if old.get("log_path") and os.path.isfile(old["log_path"]):
                os.remove(old["log_path"])
        except Exception:
            pass


def reap_finished() -> None:
    """Move exited processes into the recent table (lazy, callers invoke)."""
    with _LOCK:
        items = list(_PROCS.items())
    for name, entry in items:
        try:
            rc = entry["proc"].poll()
        except Exception:
            rc = None
        if rc is not None:
            _move_to_recent(entry, "exited" if rc == 0 else f"exited({rc})", rc)


def _duration(entry: dict) -> float:
    end = entry.get("finished_at") or time.time()
    return max(0.0, end - entry.get("started_at", end))


def list_processes() -> str:
    reap_finished()
    with _LOCK:
        running = list(_PROCS.values())
        recent = list(_RECENT.values())[-5:]
    if not running and not recent:
        return "No background processes."
    lines = []
    if running:
        lines.append("Active background processes:")
        for e in running:
            lines.append(
                f"- {e['name']}: '{e['command'][:120]}' (PID {e['proc'].pid}, "
                f"running {_duration(e):.1f}s) — process_action='logs' for output"
            )
    if recent:
        lines.append("Recently finished:")
        for e in recent:
            lines.append(
                f"- {e['name']}: {e['status']} after {_duration(e):.1f}s (PID {e['proc'].pid})"
            )
    return "\n".join(lines)


def logs(name: str, tail: int = 200) -> str:
    reap_finished()
    entry = _lookup(name)
    if entry is None:
        return f"Error: No background process named '{name}' (running or recent)."
    tail = max(1, min(int(tail or 200), 2000))
    text = ""
    if entry.get("log_path"):
        text = _tail_file(entry["log_path"], tail)
    if not text:
        try:
            text = "".join(list(entry["pump"].tail)[-tail:])
        except Exception:
            text = ""
    status = entry["status"]
    if entry.get("exit_code") is not None:
        status = f"{status}; exit {entry['exit_code']}"
    header = f"--- LOGS '{name}' ({status}, {_duration(entry):.1f}s, last {tail} lines) ---"
    if not text.strip():
        return header + "\n(no output yet)"
    return header + "\n" + text.rstrip()


def wait(name: str, timeout_ms: int = 30000) -> str:
    entry = _lookup(name)
    if entry is None:
        return f"Error: No background process named '{name}' (running or recent)."
    deadline = time.time() + max(0, int(timeout_ms)) / 1000.0
    while time.time() < deadline:
        try:
            rc = entry["proc"].poll()
        except Exception:
            rc = None
        if rc is not None:
            if entry["name"] in _PROCS:
                _move_to_recent(entry, "exited" if rc == 0 else f"exited({rc})", rc)
            tail = _tail_file(entry.get("log_path") or "", 10)
            out = f"[OK] Process '{name}' exited with code {rc} after {_duration(entry):.1f}s."
            if tail.strip():
                out += "\nLast output:\n" + tail.rstrip()
            return out
        time.sleep(0.1)
    return (
        f"[RUNNING] Process '{name}' is still running after {int(timeout_ms)}ms. "
        f"Use process_action='logs' to inspect output or 'kill' to stop it."
    )


def _port_open(port, timeout: float = 0.3) -> bool:
    try:
        s = socket.create_connection(("127.0.0.1", int(port)), timeout=timeout)
        s.close()
        return True
    except Exception:
        return False


def _entry_text(entry: dict, lines: int = 400) -> str:
    try:
        text = "".join(list(entry["pump"].tail)[-lines:])
    except Exception:
        text = ""
    if not text and entry.get("log_path"):
        text = _tail_file(entry["log_path"], lines)
    return text


def scan_ready(name: str, expect_re=None):
    """Current readiness: a TCP-probed local URL and/or an expect match."""
    reap_finished()
    entry = _lookup(name)
    if entry is None:
        return None
    text = _entry_text(entry)
    url = None
    for regex in (_URL_RE, _ADDR_PORT_RE):
        for m in regex.finditer(text):
            port = m.group(1)
            if port and _port_open(port):
                url = f"http://127.0.0.1:{port}"
                break
        if url:
            break
    return {
        "name": name,
        "url": url,
        "expect": bool(expect_re.search(text)) if expect_re else False,
        "status": entry.get("status", "running"),
        "tail": text[-400:],
    }


def wait_ready(name: str, expect_re=None, timeout_ms: int = 15000):
    """Poll until expect matches, a local port answers, the process exits, or timeout."""
    entry = _lookup(name)
    if entry is None:
        return None
    deadline = time.time() + max(0, int(timeout_ms)) / 1000.0
    while time.time() < deadline:
        try:
            if entry["proc"].poll() is not None:
                break
        except Exception:
            break
        info = scan_ready(name, expect_re)
        if info and ((expect_re and info["expect"]) or info["url"]):
            return info
        time.sleep(0.25)
    return scan_ready(name, expect_re)


def send(name: str, text) -> str:
    """Write a line to a background process's stdin."""
    entry = _lookup(name)
    if entry is None:
        return f"Error: No background process named '{name}' (running or recent)."
    if entry["name"] in _RECENT:
        return f"Process '{name}' already finished ({entry['status']})."
    proc = entry["proc"]
    try:
        if proc.stdin is None:
            return f"Error: Process '{name}' stdin is not connected."
        payload = str(text if text is not None else "")
        if not payload.endswith("\n"):
            payload += "\n"
        proc.stdin.write(payload)
        proc.stdin.flush()
        return f"[OK] Sent {len(payload)} char(s) to '{name}'."
    except Exception as e:
        return f"Error sending to '{name}': {e}"


def kill(name: str) -> str:
    entry = _lookup(name)
    if entry is None:
        return f"Error: No background process named '{name}' (running or recent)."
    if entry["name"] in _RECENT:
        return f"Process '{name}' already finished ({entry['status']})."
    proc = entry["proc"]
    # Enumerate + kill the tree while the parent is still alive: terminating
    # bash first would orphan its children (psutil can no longer find them).
    killed = kill_tree(proc)
    docker_name = entry.get("docker_name")
    if docker_name:
        try:
            subprocess.run(["docker", "rm", "-f", docker_name], capture_output=True, timeout=10)
        except Exception:
            pass
    entry["killed"] = True
    rc = None
    try:
        rc = proc.wait(timeout=TERMINATE_GRACE)
    except Exception:
        pass
    _move_to_recent(entry, "killed", rc)
    return f"[OK] Process '{name}' (PID {proc.pid}) killed — tree stopped ({killed} process(es))."


def kill_all(owner: str = None) -> int:
    reap_finished()
    with _LOCK:
        names = [
            n for n, e in _PROCS.items()
            if owner is None or e.get("owner") == owner
        ]
    for name in names:
        kill(name)
    return len(names)


def cleanup_owner(owner: str) -> int:
    """Kill every background process started by one run/chat. 0 when no owner."""
    if not owner:
        return 0
    return kill_all(owner=owner)


def resolve_owner(ctx=None) -> str:
    """Owner key for cleanup scoping: chat id when available, else workspace."""
    try:
        if ctx is not None:
            for attr in ("chat_id", "project_path"):
                val = getattr(ctx, attr, None)
                if val:
                    return str(val)
    except Exception:
        pass
    try:
        from sync.state_sync import safe_get_state
        state = safe_get_state() or {}
        return str(state.get("chat_id") or state.get("project_path") or "")
    except Exception:
        return ""


def _atexit_cleanup() -> None:
    try:
        kill_all()
    except Exception:
        pass
    with _LOCK:
        entries = list(_RECENT.values())
    for entry in entries:
        try:
            path = entry.get("log_path")
            if path and os.path.isfile(path):
                os.remove(path)
        except Exception:
            pass


atexit.register(_atexit_cleanup)
