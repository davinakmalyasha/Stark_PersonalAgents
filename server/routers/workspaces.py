"""
Workspaces + settings + rules router â€” Phase 7 moved from server/api_server.py
(workspaces:1443-1465, settings/profile:1598-1658, rules:1660-1691).
"""
import json
import os
from fastapi import APIRouter, Request; from fastapi.responses import JSONResponse

from server.store.workspace_registry import list_workspaces, add_workspace, remove_workspace, get_workspace

router = APIRouter(tags=["workspaces"])

_ROOT = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


@router.get("/api/workspaces")
async def get_workspaces():
    return JSONResponse(content={"workspaces": list_workspaces()})


@router.post("/api/workspaces")
async def create_workspace(req: Request):
    body = await req.json()
    path = body.get("path", "").strip()
    name = body.get("name", "").strip()
    if not path:
        return JSONResponse(content={"error": "Path required"}, status_code=400)
    try:
        ws = add_workspace(path, name)
        return JSONResponse(content=ws)
    except ValueError as e:
        return JSONResponse(content={"error": str(e)}, status_code=400)


@router.delete("/api/workspaces/{workspace_id}")
async def delete_workspace(workspace_id: str):
    ok = remove_workspace(workspace_id)
    return JSONResponse(content={"status": "ok" if ok else "not found"})


@router.get("/api/settings/general-profile")
async def get_general_profile():
    """Get the General mode user profile (instructions + memory)."""
    profile_path = os.path.join(_ROOT, ".deep_agents", "general_profile.json")
    default = {"user_info": {"name": "", "birthday": "", "education": ""}, "instructions": "", "memory": []}
    if os.path.isfile(profile_path):
        try:
            with open(profile_path, "r", encoding="utf-8") as f:
                return JSONResponse(content=json.load(f))
        except Exception:
            pass
    return JSONResponse(content=default)


@router.post("/api/settings/general-profile")
async def save_general_profile(req: Request):
    """Save the General mode user profile."""
    profile_path = os.path.join(_ROOT, ".deep_agents", "general_profile.json")
    body = await req.json()
    try:
        os.makedirs(os.path.dirname(profile_path), exist_ok=True)
        with open(profile_path, "w", encoding="utf-8") as f:
            json.dump(body, f, indent=2)
        return {"status": "ok"}
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)


@router.get("/api/settings/profile")
async def get_profile():
    profile_path = os.path.join(_ROOT, ".deep_agents", "user_profile.json")
    default_profile = {"user_info": {"name": "Davin Akmal Yasha"}, "global_rules": []}
    if os.path.isfile(profile_path):
        try:
            with open(profile_path, "r", encoding="utf-8") as f:
                return JSONResponse(content=json.load(f))
        except Exception:
            pass
    return JSONResponse(content=default_profile)


@router.post("/api/settings/profile")
async def save_profile(req: Request):
    profile_path = os.path.join(_ROOT, ".deep_agents", "user_profile.json")
    body = await req.json()
    try:
        os.makedirs(os.path.dirname(profile_path), exist_ok=True)
        with open(profile_path, "w", encoding="utf-8") as f:
            json.dump(body, f, indent=2)
        return {"status": "ok"}
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)


@router.get("/api/workspaces/{workspace_id}/rules")
async def get_workspace_rules(workspace_id: str):
    ws = get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    rules_path = os.path.join(_ROOT, ".deep_agents", "rules", f"{workspace_id}.json")
    default_rules = {"stack": {}, "workspace_rules": []}
    if os.path.isfile(rules_path):
        try:
            with open(rules_path, "r", encoding="utf-8") as f:
                return JSONResponse(content=json.load(f))
        except Exception:
            pass
    return JSONResponse(content=default_rules)


@router.post("/api/workspaces/{workspace_id}/rules")
async def save_workspace_rules(workspace_id: str, req: Request):
    ws = get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    rules_path = os.path.join(_ROOT, ".deep_agents", "rules", f"{workspace_id}.json")
    body = await req.json()
    try:
        os.makedirs(os.path.dirname(rules_path), exist_ok=True)
        with open(rules_path, "w", encoding="utf-8") as f:
            json.dump(body, f, indent=2)
        return {"status": "ok"}
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)



