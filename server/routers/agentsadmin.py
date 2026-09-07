"""
Super-operator admin panel for AgentsProxy (multi-tenant bot fleet).

Transport: the LangChain frontend talks to THIS router; the router holds
AGENTSPROXY_ADMIN_KEY (server-side only, never shipped to the browser)
and calls the proxy's MCP endpoint (Streamable HTTP + Bearer admin key).
"""
import json
import os
import urllib.error
import urllib.request

from fastapi import APIRouter
from fastapi.responses import JSONResponse

router = APIRouter(prefix="/api/agentsadmin", tags=["agentsadmin"])


def _proxy_url() -> str:
    return (os.environ.get("AGENTSPROXY_URL") or "http://localhost:8080").rstrip("/")


def _admin_key() -> str:
    return os.environ.get("AGENTSPROXY_ADMIN_KEY") or ""


def _call(method: str, params: dict) -> dict:
    url = _proxy_url() + "/v1/mcp"
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, data=body, method="POST", headers={
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "Authorization": "Bearer " + _admin_key(),
    })
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            data = resp.read().decode("utf-8", "replace")
            payload = json.loads(data) if data.strip() else {}
    except urllib.error.HTTPError as e:
        return {"ok": False, "error": f"HTTP {e.code}: {e.reason}"}
    except Exception as e:  # network / timeout
        return {"ok": False, "error": str(e)[:200]}
    err = payload.get("error")
    if err:
        return {"ok": False, "error": (err.get("message") or str(err))[:300]}
    content = (payload.get("result") or {}).get("content") or []
    text = "\n".join(str(b.get("text", "")) for b in content if isinstance(b, dict) and b.get("type") == "text")
    return {"ok": True, "text": text}


def _health() -> JSONResponse:
    if not _admin_key():
        return JSONResponse(content={"ok": False, "error": "AGENTSPROXY_ADMIN_KEY not set on this server"}, status_code=503)
    res = _call("initialize", {})
    if not res.get("ok"):
        return JSONResponse(content=res, status_code=503)
    return JSONResponse(content=_call("tools/call", {
        "name": "admin_proxy_health",
        "arguments": {},
    }))


@router.get("/health")
async def health():
    return _health()


@router.get("/clients")
async def clients():
    res = _call("tools/call", {"name": "admin_clients_list", "arguments": {}})
    return JSONResponse(content=res)


@router.get("/bots")
async def bots(tenant_id: str = ""):
    args = {"tenant_id": tenant_id} if tenant_id else {}
    res = _call("tools/call", {"name": "admin_bots_list", "arguments": args})
    return JSONResponse(content=res)


@router.post("/bots/{bot_id}/disable")
async def disable_bot(bot_id: str):
    res = _call("tools/call", {"name": "admin_bot_disable", "arguments": {"bot_id": bot_id}})
    return JSONResponse(content=res)


@router.get("/usage")
async def usage(tenant_id: str):
    if not tenant_id:
        return JSONResponse(content={"ok": False, "error": "tenant_id required"}, status_code=400)
    res = _call("tools/call", {"name": "admin_client_usage", "arguments": {"tenant_id": tenant_id}})
    return JSONResponse(content=res)


@router.get("/dead-reminders")
async def dead_reminders(tenant_id: str = ""):
    args = {"tenant_id": tenant_id} if tenant_id else {}
    res = _call("tools/call", {"name": "admin_reminders_dead", "arguments": args})
    return JSONResponse(content=res)
