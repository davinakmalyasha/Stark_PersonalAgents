"""
House API router — Phase 7 moved from server/api_server.py:1323-1399.
"""
from fastapi import APIRouter
from fastapi.responses import JSONResponse
from pydantic import BaseModel

from server.house import service as hm

router = APIRouter(prefix="/api/house", tags=["house"])


class HouseStatePatch(BaseModel):
    state: dict = {}


@router.get("/state")
async def house_state():
    """Full house state: rooms, devices, scenes, energy, security."""
    try:
        return JSONResponse(content=hm.get_house_state())
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)


@router.get("/devices/{device_id}")
async def house_device(device_id: str):
    device = hm.get_device(device_id)
    if device is None:
        return JSONResponse(content={"error": "Device not found"}, status_code=404)
    return JSONResponse(content=device)


@router.patch("/devices/{device_id}")
async def house_set_device(device_id: str, req: HouseStatePatch):
    """Write a state patch to one device (validated + clamped by the manager)."""
    result = hm.set_device(device_id, req.state)
    if not result.get("ok"):
        status = 404 if "Unknown device" in (result.get("error") or "") else 400
        return JSONResponse(content={"error": result.get("error")}, status_code=status)
    return JSONResponse(content=result.get("device"))


@router.post("/rooms/{room_id}")
async def house_set_room(room_id: str, req: HouseStatePatch):
    """Apply one patch to every device in a room that supports those keys."""
    result = hm.set_room_state(room_id, req.state)
    if result.get("error"):
        return JSONResponse(content={"error": result["error"]}, status_code=404)
    return JSONResponse(content={"applied": result.get("applied", []),
                                 "skipped": result.get("skipped", [])})


@router.post("/scenes/{scene_id}/run")
async def house_run_scene(scene_id: str):
    """Activate a scene (applies all its device actions)."""
    result = hm.run_scene(scene_id)
    if not result.get("ok"):
        return JSONResponse(content={"error": result.get("error")}, status_code=404)
    scene = result.get("scene") or {}
    return JSONResponse(content={
        "sceneId": scene_id,
        "name": scene.get("name", ""),
        "applied": result.get("applied", []),
        "skipped": result.get("skipped", []),
    })


@router.get("/history")
async def house_history(device_id: str = "", hours: int = 24):
    """Mock usage history for one device (hourly kWh series)."""
    if not device_id:
        return JSONResponse(content={"error": "device_id is required"}, status_code=400)
    return JSONResponse(content=hm.get_device_history(device_id, hours))


@router.post("/reset")
async def house_reset():
    """Reseed the mock house state (demo/test helper)."""
    result = hm.reset_house_state()
    if not result.get("ok"):
        return JSONResponse(content={"error": result.get("error")}, status_code=400)
    return JSONResponse(content=result)
