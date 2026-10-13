"""tools.mcp.mcp_bridge — minimal MCP client + manager (zero new dependencies).

MCP over stdio is line-delimited JSON-RPC over a child process's stdin/stdout;
Streamable HTTP is JSON-RPC POSTs with an optional bearer token. We implement
just enough of the protocol (initialize -> notifications/initialized ->
tools/list -> tools/call) to stay dependency-free and robust in this runtime.

Lifecycle (codex-style):
  - enabled servers are spawned + initialized concurrently at API startup
    (background thread per server) and kept alive for the whole process
  - tools/list is fetched ONCE per server; schemas are cached
  - server `instructions` is kept for prompt guidance
  - calls are serialized per server (single JSON-RPC session), bounded results
  - shutdown() terminates child processes

Tool names are fully-qualified as ``mcp__<server>__<tool>`` (double-underscore
delimiter is safe for OpenAI-compatible function-name validation, unlike "/").
"""
import itertools
import json
import os
import re
import shutil
import subprocess
import threading
import time
import urllib.error
import urllib.request

from sync.state_sync import safe_update_state, safe_append_live_log

_MCP_PROTOCOL_VERSION = "2025-06-18"
_CLIENT_INFO = {"name": "stark-harness", "version": "2.0.0"}
_CONFIG_FILE = "mcp_servers.json"
_MAX_TOOLS_PER_SERVER = 500
_INSTRUCTIONS_CAP = 512

_HR = os.environ.get("HARNESS_ROOT") or os.path.dirname(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
)


# ── Small helpers ─────────────────────────────────────────────────────────────

#: Execution scopes are agent display names ("Dev", "General", "Orchestrator");
#: schema scopes are the canonical lowercase keys from _schemas._SCOPE_MAP
#: ("developer", "general", "orchestrator"). Normalize both sides so a server
#: tagged "developer" is callable when Dev executes (and vice versa).
_SCOPE_ALIASES = {"dev": "developer"}


def _canonical_scope(scope: str) -> str:
    s = str(scope or "").strip().lower()
    return _SCOPE_ALIASES.get(s, s)


def _config_path() -> str:
    d = os.path.join(_HR, ".deep_agents")
    try:
        os.makedirs(d, exist_ok=True)
    except Exception:
        pass
    return os.path.join(d, _CONFIG_FILE)


def _sanitize_name(s: str, cap: int = 48) -> str:
    s = re.sub(r"[^a-zA-Z0-9_-]", "_", s)
    s = s[:cap].strip("_")
    return s or "x"


def _fq_tool_name(server: str, tool: str) -> str:
    return f"mcp__{_sanitize_name(server)}__{_sanitize_name(tool)}"


def _parse_fq(fq: str) -> tuple[str, str]:
    parts = fq.split("__")
    if not fq.startswith("mcp__") or len(parts) < 3:
        raise ValueError(f"invalid MCP tool name '{fq}'")
    return parts[1], "__".join(parts[2:])


def _log(line: str) -> None:
    try:
        safe_append_live_log(line)
    except Exception:
        print(line)


# ── Transports ────────────────────────────────────────────────────────────────

class _StdioTransport:
    """Line-delimited JSON-RPC over a child process stdin/stdout."""

    def __init__(self, cfg: dict):
        self._cmd = str(cfg.get("command", "")).strip()
        self._args = [str(a) for a in cfg.get("args", [])]
        self._env = dict(cfg.get("env") or {})
        self._env_vars = cfg.get("env_vars") or []
        self._cwd = cfg.get("cwd")
        self._proc = None
        self._stderr_tail: list[str] = []
        self._io_lock = threading.Lock()
        self._cond = threading.Condition()
        self._pending: dict[int, dict] = {}
        self._ids = itertools.count(1)

    def connect(self, timeout: float) -> None:
        if not self._cmd:
            raise ValueError("stdio MCP server requires 'command'")
        # Windows CreateProcess does not apply PATHEXT, so bare "npx"/"uvx"
        # must resolve to the .cmd/.exe shim before Popen.
        cmd = shutil.which(self._cmd) or self._cmd
        env = dict(os.environ)
        # env_clear-style whitelist, but always keep the minimal Windows vars
        # CreateProcess/Python need (an empty block raises WinError 87, and
        # without SYSTEMROOT the child Python dies in hash randomization).
        # Windows env names are case-insensitive, so compare lowercased.
        essential = {
            "systemroot", "systemdrive", "windir", "comspec", "pathext",
            "path", "temp", "tmp", "userprofile", "home",
            "localappdata", "appdata", "processor_architecture",
        }
        for k in list(env):
            if k.lower() not in essential and k not in self._env and k not in self._env_vars:
                env.pop(k, None)
        env.update(self._env)
        flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        self._proc = subprocess.Popen(
            [cmd] + self._args,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            bufsize=1,
            cwd=self._cwd,
            env=env,
            creationflags=flags,
        )
        threading.Thread(target=self._reader, daemon=True).start()
        threading.Thread(target=self._stderr_reader, daemon=True).start()

    def _reader(self) -> None:
        try:
            for line in self._proc.stdout:
                line = line.strip()
                if not line:
                    continue
                try:
                    msg = json.loads(line)
                except Exception:
                    continue
                if not isinstance(msg, dict):
                    continue
                if "id" in msg:
                    with self._cond:
                        entry = self._pending.pop(msg.get("id"), None)
                    if entry:
                        if "error" in msg:
                            entry["error"] = (msg["error"] or {}).get("message", str(msg["error"]))
                        else:
                            entry["result"] = msg.get("result")
                        entry["event"].set()
        except Exception:
            pass
        finally:
            with self._cond:
                self._pending.clear()
                self._cond.notify_all()

    def _stderr_reader(self) -> None:
        try:
            for line in self._proc.stderr:
                line = line.rstrip("\n")
                if not line:
                    continue
                self._stderr_tail.append(line[:300])
                if len(self._stderr_tail) > 8:
                    self._stderr_tail.pop(0)
        except Exception:
            pass

    def request(self, method: str, params: dict, timeout: float):
        rid = next(self._ids)
        entry = {"event": threading.Event(), "result": None, "error": None}
        with self._cond:
            self._pending[rid] = entry
        msg = {"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}}
        with self._io_lock:
            self._proc.stdin.write(json.dumps(msg) + "\n")
            self._proc.stdin.flush()
        if not entry["event"].wait(timeout):
            with self._cond:
                self._pending.pop(rid, None)
            raise TimeoutError(f"{method} timed out after {timeout}s")
        if entry["error"]:
            raise RuntimeError(entry["error"])
        return entry["result"]

    def notify(self, method: str) -> None:
        msg = {"jsonrpc": "2.0", "method": method}
        try:
            with self._io_lock:
                self._proc.stdin.write(json.dumps(msg) + "\n")
                self._proc.stdin.flush()
        except Exception:
            pass

    def close(self) -> None:
        if self._proc is not None:
            try:
                self._proc.terminate()
            except Exception:
                pass
            try:
                self._proc.wait(timeout=3)
            except Exception:
                try:
                    self._proc.kill()
                except Exception:
                    pass
            self._proc = None

    def tail_error(self) -> str:
        return "; ".join(self._stderr_tail)


class _HttpTransport:
    """Streamable HTTP JSON-RPC via stdlib urllib (bearer token optional)."""

    def __init__(self, cfg: dict):
        self._url = str(cfg.get("url", "")).strip()
        self._bearer = ""
        env_var = cfg.get("bearer_token_env_var")
        if env_var:
            self._bearer = os.environ.get(str(env_var), "") or ""
        self._headers = {str(k): str(v) for k, v in (cfg.get("http_headers") or {}).items()}
        self._session_id = None
        self._ids = itertools.count(1)

    def _post(self, method: str, params: dict, timeout: float, delete: bool = False):
        rid = next(self._ids)
        headers = {
            "Accept": "application/json, text/event-stream",
            "Content-Type": "application/json",
        }
        if self._bearer:
            headers["Authorization"] = "Bearer " + self._bearer
        headers.update(self._headers)
        if self._session_id:
            headers["Mcp-Session-Id"] = self._session_id
        body = json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}})
        req = urllib.request.Request(self._url, data=body.encode("utf-8"), headers=headers, method="DELETE" if delete else "POST")
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                session = resp.headers.get("Mcp-Session-Id")
                if session:
                    self._session_id = session
                ctype = resp.headers.get("Content-Type", "")
                data = resp.read().decode("utf-8", "replace")
        except urllib.error.HTTPError as e:
            raise RuntimeError(f"HTTP {e.code}: {e.reason}") from e
        if not data.strip():
            return None
        parsed = self._parse(data, ctype)
        if isinstance(parsed, dict):
            if "result" in parsed:
                return parsed["result"]
            if "error" in parsed:
                raise RuntimeError((parsed["error"] or {}).get("message", str(parsed["error"])))
        return parsed

    @staticmethod
    def _parse(data: str, ctype: str):
        if "text/event-stream" in ctype:
            for line in data.splitlines():
                line = line.strip()
                if line.startswith("data:"):
                    payload = line[5:].strip()
                    if payload:
                        try:
                            return json.loads(payload)
                        except Exception:
                            continue
            return None
        try:
            return json.loads(data)
        except Exception:
            return {"content": data[:8000]}

    def connect(self, timeout: float) -> None:
        if not self._url:
            raise ValueError("http MCP server requires 'url'")
        self._post("initialize", {}, timeout)

    def request(self, method: str, params: dict, timeout: float):
        return self._post(method, params, timeout)

    def notify(self, method: str) -> None:
        try:
            self._post(method, {}, 10)
        except Exception:
            pass

    def close(self) -> None:
        if self._session_id:
            try:
                self._post("", {}, 10, delete=True)
            except Exception:
                pass
            self._session_id = None


# ── Client (one per server) ───────────────────────────────────────────────────

class _McpClient:
    def __init__(self, server_name: str, cfg: dict):
        self.name = server_name
        self.cfg = cfg
        self.status = "disabled" if not cfg.get("enabled", True) else "starting"
        self.error = ""
        self.tools: list[dict] = []
        self.instructions = ""
        self.fq_to_tool: dict[str, str] = {}
        self._ready = threading.Event()
        self._call_lock = threading.Lock()
        self._transport = None

    def _make_transport(self):
        if self.cfg.get("url"):
            return _HttpTransport(self.cfg)
        return _StdioTransport(self.cfg)

    def start(self) -> None:
        startup_timeout = float(self.cfg.get("startup_timeout_sec", 10) or 10)
        transport = self._make_transport()
        self._transport = transport
        transport.connect(startup_timeout)
        result = transport.request(
            "initialize",
            {
                "protocolVersion": _MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": _CLIENT_INFO,
            },
            startup_timeout,
        ) or {}
        if isinstance(result, dict) and result.get("instructions"):
            self.instructions = str(result["instructions"])[:_INSTRUCTIONS_CAP]
        transport.notify("notifications/initialized")
        cursor = None
        while True:
            params = {"cursor": cursor} if cursor else {}
            listing = transport.request("tools/list", params, startup_timeout) or {}
            for t in listing.get("tools", []):
                if len(self.tools) >= _MAX_TOOLS_PER_SERVER:
                    break
                self.tools.append(t)
            cursor = listing.get("nextCursor")
            if not cursor:
                break
        self.fq_to_tool = {_fq_tool_name(self.name, t.get("name", "")): t.get("name", "") for t in self.tools}
        self.status = "connected"

    def call_tool(self, tool: str, args: dict, timeout: float) -> str:
        if self._transport is None:
            raise RuntimeError("not connected")
        tool_timeout = float(self.cfg.get("tool_timeout_sec", 60) or 60)
        if timeout is None:
            timeout = tool_timeout
        with self._call_lock:
            result = self._transport.request("tools/call", {"name": tool, "arguments": args or {}}, timeout) or {}
        if result.get("isError"):
            text = _content_text(result.get("content", []))
            raise RuntimeError(text or "MCP tool returned an error")
        return _content_text(result.get("content", []))

    def close(self) -> None:
        try:
            if self._transport is not None:
                self._transport.close()
        except Exception:
            pass
        self._transport = None
        self._ready.set()

    def mark_failed(self, err: str) -> None:
        self.status = "failed"
        self.error = str(err)[:300]
        if isinstance(self._transport, _StdioTransport):
            tail = self._transport.tail_error()
            if tail and tail not in self.error:
                self.error = (self.error + " | " + tail)[:400]
        self._ready.set()


def _content_text(content: list) -> str:
    parts = []
    for block in content or []:
        if not isinstance(block, dict):
            continue
        btype = block.get("type")
        if btype in ("text",):
            text = block.get("text", "")
            if text:
                parts.append(text)
        elif btype == "image":
            parts.append(f"[MCP image block: {block.get('mimeType', 'unknown')}]")
        elif btype == "resource":
            parts.append(str(block.get("resource", "")))
        else:
            parts.append(str(block))
    return "\n".join(parts) if parts else "[MCP tool returned no text content]"


# ── Manager ───────────────────────────────────────────────────────────────────

class McpManager:
    def __init__(self):
        self._lock = threading.Lock()
        self._clients: dict[str, _McpClient] = {}
        self._started = False

    # config ──
    def load_config(self) -> dict:
        path = _config_path()
        try:
            if os.path.isfile(path):
                with open(path, "r", encoding="utf-8") as f:
                    data = json.load(f)
                servers = data.get("servers")
                if isinstance(servers, dict):
                    return {"servers": servers}
        except Exception as e:
            _log(f"[MCP] config load error: {e}")
        return {"servers": {}}

    def save_config(self, servers: dict) -> None:
        path = _config_path()
        with open(path, "w", encoding="utf-8") as f:
            json.dump({"servers": servers}, f, indent=2)

    # lifecycle ──
    def start_all(self, background: bool = True) -> None:
        with self._lock:
            if self._started:
                return
            self._started = True
            cfg = self.load_config()
            for name, scfg in cfg.get("servers", {}).items():
                client = _McpClient(name, scfg)
                self._clients[name] = client
        to_connect = [c for c in self._clients.values() if c.status == "starting"]
        if background:
            for client in to_connect:
                threading.Thread(target=self._connect_one, args=(client,), daemon=True).start()
        else:
            for client in to_connect:
                self._connect_one(client)
        self._push_status()

    def _connect_one(self, client: _McpClient) -> None:
        _log(f"[MCP] connecting '{client.name}'...")
        try:
            client.start()
            _log(f"[MCP] '{client.name}' connected ({len(client.tools)} tools)")
        except Exception as e:
            client.mark_failed(str(e))
            _log(f"[MCP] '{client.name}' FAILED: {client.error}")
        self._push_status()

    def ensure_started(self, wait_sec: float = 3.0) -> None:
        # Never spawn servers implicitly: connections happen only when the app
        # explicitly called start_all() (API startup). Queries from schema
        # helpers run at module import time and must stay side-effect free.
        with self._lock:
            if not self._started:
                return
        deadline = time.time() + wait_sec
        for client in list(self._clients.values()):
            if client.status not in ("starting",):
                continue
            remaining = deadline - time.time()
            if remaining <= 0:
                return
            client._ready.wait(remaining)

    def shutdown(self) -> None:
        for client in list(self._clients.values()):
            try:
                client.close()
            except Exception:
                pass
        self._clients.clear()
        self._started = False

    # queries (called by _schemas hooks during LLM tool building) ──
    def _scope_matches(self, client: _McpClient, scope: str) -> bool:
        scopes = {_canonical_scope(s) for s in client.cfg.get("scopes", [])}
        return not scopes or (not scope or _canonical_scope(scope) in scopes)

    def tool_defs_for_scope(self, scope: str = "") -> list[dict]:
        self.ensure_started()
        out = []
        seen = set()
        for name in sorted(self._clients):
            client = self._clients[name]
            if client.status != "connected" or not self._scope_matches(client, scope):
                continue
            enabled_tools = set(client.cfg.get("enabled_tools", []))
            disabled_tools = set(client.cfg.get("disabled_tools", []))
            for t in client.tools:
                tname = t.get("name", "")
                if enabled_tools and tname not in enabled_tools:
                    continue
                if tname in disabled_tools:
                    continue
                fq = _fq_tool_name(name, tname)
                if fq in seen:
                    continue
                seen.add(fq)
                out.append(_tool_def(fq, t))
        return out

    def tool_names_for_scope(self, scope: str = "") -> list[str]:
        return [d["function"]["name"] for d in self.tool_defs_for_scope(scope)]

    def instructions_for_scope(self, scope: str = "") -> list[tuple[str, str]]:
        self.ensure_started()
        out = []
        for name in sorted(self._clients):
            client = self._clients[name]
            if client.status == "connected" and self._scope_matches(client, scope) and client.instructions:
                out.append((name, client.instructions))
        return out

    # execution (called by the registry hook) ──
    def call(self, fqname: str, args: dict, scope: str = "", timeout: float = None) -> str:
        server, tool = _parse_fq(fqname)
        with self._lock:
            client = self._clients.get(server)
        if client is None:
            return f"Error: unknown MCP server '{server}'"
        if client.status != "connected":
            return f"Error: MCP server '{server}' not connected (status: {client.status}{': ' + client.error if client.error else ''})"
        if not self._scope_matches(client, scope):
            return f"Error: MCP tool '{fqname}' is not exposed to scope '{scope or 'agent'}'"
        if client.cfg.get("approval") == "prompt" and not _confirm(server, tool, args):
            return f"Error: MCP tool call {server}/{tool} was not approved."
        try:
            result = client.call_tool(tool, args, timeout)
        except TimeoutError as e:
            return f"Error: {e}"
        except Exception as e:
            return f"Error: MCP call {server}/{tool} failed: {e}"
        try:
            from tools._runtime import auto_offload_result, _tool_cap
            from tools.MainTool._shared import _log_tool
            bounded = auto_offload_result(result, "mcp", max_chars=_tool_cap("mcp"))
            _log_tool(fqname, args, bounded)
        except Exception:
            try:
                bounded = result[: _tool_cap("mcp")]
            except Exception:
                bounded = result[:25000]
        return bounded

    # status / management (API endpoints) ──
    def status_snapshot(self) -> list[dict]:
        out = []
        for name in sorted(self._clients):
            client = self._clients[name]
            out.append({
                "name": name,
                "status": client.status,
                "transport": "http" if client.cfg.get("url") else "stdio",
                "error": client.error,
                "tool_count": len(client.tools),
                "scopes": [str(s) for s in client.cfg.get("scopes", [])],
            })
        return out

    def upsert_server(self, name: str, cfg: dict) -> list[dict]:
        if not re.match(r"^[a-zA-Z0-9_-]+$", name):
            raise ValueError("server name must match ^[a-zA-Z0-9_-]+$")
        with self._lock:
            servers = self.load_config().get("servers", {})
            servers[name] = cfg
            self.save_config(servers)
            old = self._clients.pop(name, None)
            if old is not None:
                old.close()
            client = _McpClient(name, cfg)
            self._clients[name] = client
        if client.status == "starting":
            threading.Thread(target=self._connect_one, args=(client,), daemon=True).start()
        self._push_status()
        return self.status_snapshot()

    def remove_server(self, name: str) -> list[dict]:
        with self._lock:
            servers = self.load_config().get("servers", {})
            servers.pop(name, None)
            self.save_config(servers)
            old = self._clients.pop(name, None)
            if old is not None:
                old.close()
        self._push_status()
        return self.status_snapshot()

    def restart_server(self, name: str) -> list[dict]:
        with self._lock:
            servers = self.load_config().get("servers", {})
            cfg = servers.get(name)
            if cfg is None:
                raise ValueError(f"server '{name}' not configured")
            old = self._clients.pop(name, None)
            if old is not None:
                old.close()
            client = _McpClient(name, cfg)
            self._clients[name] = client
        self._connect_one(client)
        return self.status_snapshot()

    def _push_status(self) -> None:
        try:
            safe_update_state({"mcp_servers": self.status_snapshot()})
        except Exception:
            pass


_manager: McpManager = None
_manager_lock = threading.Lock()


def get_manager() -> McpManager:
    global _manager
    if _manager is None:
        with _manager_lock:
            if _manager is None:
                _manager = McpManager()
    return _manager


def _confirm(server: str, tool: str, args: dict) -> bool:
    try:
        from tools.AddOnTool.orchestrationTools import ask_user
        answer = ask_user(
            [{
                "question": f"Approve MCP tool call {server}/{tool} with {json.dumps(args)[:400]}?",
                "options": ["Approve", "Deny"],
            }],
            timeout=180,
        )
        return "approve" in str(answer).lower()
    except Exception:
        return False


def _tool_def(fq: str, t: dict) -> dict:
    description = (str(t.get("description", "") or "").strip())[:400]
    schema = t.get("inputSchema") or {}
    if not isinstance(schema, dict):
        schema = {}
    properties = schema.get("properties") if isinstance(schema.get("properties"), dict) else {}
    required = [str(r) for r in schema.get("required", []) if isinstance(r, str)]
    return {
        "type": "function",
        "function": {
            "name": fq,
            "description": description or f"MCP tool {fq}.",
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
            },
        },
    }


# ── Hooks consumed by _schemas / _registry (lazy imports avoid cycles) ────────

def mcp_tool_defs_for_scope(scope: str = "") -> list[dict]:
    try:
        return get_manager().tool_defs_for_scope(scope)
    except Exception:
        return []


def mcp_tool_names_for_scope(scope: str = "") -> list[str]:
    try:
        return get_manager().tool_names_for_scope(scope)
    except Exception:
        return []


def mcp_instructions_for_scope(scope: str = "") -> list[tuple[str, str]]:
    try:
        return get_manager().instructions_for_scope(scope)
    except Exception:
        return []


def mcp_execute(fqname: str, args: dict, scope: str = "") -> str:
    """Registry fallback: execute a fully-qualified MCP tool."""
    return get_manager().call(fqname, args, scope)


def mcp_start_all(background: bool = True) -> None:
    get_manager().start_all(background=background)


def mcp_shutdown() -> None:
    try:
        get_manager().shutdown()
    except Exception:
        pass