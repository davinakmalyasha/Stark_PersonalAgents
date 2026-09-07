"""
UI state persistence router â€” Phase 7 moved from server/api_server.py:670-695.
"""
import json
import os
from fastapi import APIRouter, Request; from fastapi.responses import JSONResponse

router = APIRouter(tags=["ui"])

_ROOT = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _ui_state_path() -> str:
    return os.path.join(_ROOT, ".deep_agents", "ui_state.json")


@router.get("/api/ui/state")
async def get_ui_state():
    try:
        with open(_ui_state_path(), "r", encoding="utf-8") as f:
            return JSONResponse(content=json.load(f))
    except Exception:
        return JSONResponse(content={})


@router.put("/api/ui/state")
async def put_ui_state(request: Request):
    try:
        body = await request.json()
    except Exception:
        return JSONResponse(content={"error": "invalid json"}, status_code=400)
    try:
        os.makedirs(os.path.dirname(_ui_state_path()), exist_ok=True)
        with open(_ui_state_path(), "w", encoding="utf-8") as f:
            json.dump(body, f, indent=2)
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)
    return JSONResponse(content={"status": "ok"})



