"""
MCP server lifecycle router â€” Phase 7 moved from server/api_server.py:700-762.
"""
from fastapi import APIRouter; from fastapi.responses import JSONResponse
from pydantic import BaseModel

router = APIRouter(prefix="/api/mcp", tags=["mcp"])


class McpServerUpdate(BaseModel):
    command: str = ""
    args: list[str] = []
    url: str = ""
    env: dict[str, str] = {}
    env_vars: list[str] = []
    cwd: str = ""
    scopes: list[str] = []
    enabled_tools: list[str] = []
    disabled_tools: list[str] = []
    approval: str = "auto"  # auto | prompt
    instructions: str = ""
    startup_timeout_sec: float = 10.0
    tool_timeout_sec: float = 60.0


def _mcp_manager():
    from tools.mcp.mcp_bridge import get_manager
    return get_manager()


@router.get("/servers")
async def list_mcp_servers():
    """List configured servers and their live connection status."""
    mgr = _mcp_manager()
    status = {s["name"]: s for s in mgr.status_snapshot()}
    out = []
    for name, cfg in mgr.load_config().get("servers", {}).items():
        merged = dict(cfg)
        merged["name"] = name
        merged["status"] = status.get(name, {}).get("status", "stopped")
        merged["error"] = status.get(name, {}).get("error", "")
        merged["tool_count"] = status.get(name, {}).get("tool_count", 0)
        merged["transport"] = "http" if cfg.get("url") else "stdio"
        out.append(merged)
    return JSONResponse(content={"servers": out})


@router.post("/servers/{server_name}")
async def upsert_mcp_server(server_name: str, update: McpServerUpdate):
    """Create or replace a server config; restarts it if configured."""
    try:
        snapshot = _mcp_manager().upsert_server(server_name, update.model_dump(exclude_none=True))
        return JSONResponse(content={"servers": snapshot})
    except ValueError as e:
        return JSONResponse(content={"error": str(e)}, status_code=400)


@router.delete("/servers/{server_name}")
async def delete_mcp_server(server_name: str):
    """Remove a server config and disconnect it."""
    snapshot = _mcp_manager().remove_server(server_name)
    return JSONResponse(content={"servers": snapshot})


@router.post("/servers/{server_name}/restart")
async def restart_mcp_server(server_name: str):
    """Disconnect and reconnect a single server (pick up config changes)."""
    try:
        snapshot = _mcp_manager().restart_server(server_name)
        return JSONResponse(content={"servers": snapshot})
    except ValueError as e:
        return JSONResponse(content={"error": str(e)}, status_code=404)



