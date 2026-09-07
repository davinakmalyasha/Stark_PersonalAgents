"""
House manager — Phase 7 moved from Desktop/house_manager.py.
Mock smart-home state file .deep_agents/house_state.json + adapter seam.
"""
import os
import json
import threading
import time
from datetime import datetime

_ROOT = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HOUSE_STATE_PATH = os.path.join(_ROOT, ".deep_agents", "house_state.json")

_ADAPTER = None
_lock = threading.RLock()

_DEVICE_CAPABILITIES = {
    "light": ["on", "brightness"],
    "thermostat": ["mode", "target_temperature"],
    "lock": ["locked"],
    "plug": ["on"],
    "blinds": ["position"],
    "camera": ["recording"],
    "sensor": ["alert"],
}

_STATE_LIMITS = {
    "brightness": (0, 100),
    "target_temperature": (50, 90),
    "position": (0, 100),
}

_THERMOSTAT_MODES = ("heat", "cool", "auto", "off")


def _seed_devices():
    return {
        "living_light_1": {"name": "Living Room Light", "type": "light", "room": "living", "state": {"on": True, "brightness": 70}},
        "living_thermostat": {"name": "Living Room Thermostat", "type": "thermostat", "room": "living", "state": {"mode": "auto", "target_temperature": 72}},
        "living_tv_plug": {"name": "TV Plug", "type": "plug", "room": "living", "state": {"on": True}},
        "living_blinds": {"name": "Living Room Blinds", "type": "blinds", "room": "living", "state": {"position": 40}},
        "kitchen_light_1": {"name": "Kitchen Light", "type": "light", "room": "kitchen", "state": {"on": False, "brightness": 80}},
        "kitchen_light_2": {"name": "Kitchen Counter Light", "type": "light", "room": "kitchen", "state": {"on": False, "brightness": 60}},
        "kitchen_plug_1": {"name": "Coffee Maker Plug", "type": "plug", "room": "kitchen", "state": {"on": False}},
        "kitchen_sensor": {"name": "Kitchen Motion Sensor", "type": "sensor", "room": "kitchen", "state": {"alert": False}},
        "bedroom_light_1": {"name": "Bedroom Light", "type": "light", "room": "bedroom", "state": {"on": False, "brightness": 50}},
        "bedroom_thermostat": {"name": "Bedroom Thermostat", "type": "thermostat", "room": "bedroom", "state": {"mode": "cool", "target_temperature": 68}},
        "bedroom_lock": {"name": "Bedroom Lock", "type": "lock", "room": "bedroom", "state": {"locked": True}},
        "bathroom_light_1": {"name": "Bathroom Light", "type": "light", "room": "bathroom", "state": {"on": False, "brightness": 40}},
        "bathroom_plug": {"name": "Bathroom Fan Plug", "type": "plug", "room": "bathroom", "state": {"on": False}},
        "garage_light_1": {"name": "Garage Light", "type": "light", "room": "garage", "state": {"on": False, "brightness": 100}},
        "garage_lock": {"name": "Garage Door Lock", "type": "lock", "room": "garage", "state": {"locked": True}},
        "garage_camera": {"name": "Garage Camera", "type": "camera", "room": "garage", "state": {"recording": True}},
        "front_door_lock": {"name": "Front Door Lock", "type": "lock", "room": "living", "state": {"locked": True}},
        "front_camera": {"name": "Front Camera", "type": "camera", "room": "living", "state": {"recording": True}},
        "garage_sensor": {"name": "Garage Motion Sensor", "type": "sensor", "room": "garage", "state": {"alert": False}},
    }


def _seed_scenes():
    return {
        "movie_night": {"name": "Movie Night", "actions": {"living_light_1": {"on": False}, "living_blinds": {"position": 0}}},
        "good_morning": {"name": "Good Morning", "actions": {"living_light_1": {"on": True, "brightness": 60}, "kitchen_light_1": {"on": True, "brightness": 80}}},
        "goodnight": {"name": "Goodnight", "actions": {"living_light_1": {"on": False}, "bedroom_light_1": {"on": False}, "garage_lock": {"locked": True}, "front_door_lock": {"locked": True}}},
        "vacation": {"name": "Vacation Mode", "actions": {"living_light_1": {"on": False}, "garage_lock": {"locked": True}, "front_door_lock": {"locked": True}, "garage_camera": {"recording": True}}},
        "home": {"name": "I'm Home", "actions": {"living_light_1": {"on": True, "brightness": 70}, "front_door_lock": {"locked": False}}},
    }


def _default_state():
    return {
        "rooms": ["living", "kitchen", "bedroom", "bathroom", "garage"],
        "devices": _seed_devices(),
        "scenes": _seed_scenes(),
        "energy": {"today_kwh": 3.2, "month_kwh": 82.5, "cost_today": 0.58, "cost_month": 14.9},
        "security": {"armed": False, "alerts": [], "cameras": ["garage_camera", "front_camera"]},
        "history": {},
        "updatedAt": datetime.now().isoformat(),
    }


def _load_state() -> dict:
    try:
        with open(HOUSE_STATE_PATH, "r", encoding="utf-8") as f:
            data = json.load(f)
        if isinstance(data, dict) and "devices" in data:
            return data
    except Exception:
        pass
    state = _default_state()
    _save_state(state)
    return state


def _save_state(state: dict) -> None:
    try:
        os.makedirs(os.path.dirname(HOUSE_STATE_PATH), exist_ok=True)
        with open(HOUSE_STATE_PATH, "w", encoding="utf-8") as f:
            json.dump(state, f, indent=2, ensure_ascii=False)
    except Exception as e:
        print(f"[HOUSE] save failed: {e}")


def _validate_patch(dtype: str, patch: dict) -> tuple[bool, dict, str]:
    if not isinstance(patch, dict):
        return False, {}, "patch must be an object"
    caps = _DEVICE_CAPABILITIES.get(dtype, [])
    valid = {}
    for k, v in patch.items():
        if k not in caps:
            continue
        if k == "on" or k == "locked" or k == "recording" or k == "alert":
            valid[k] = bool(v)
        else:
            lo, hi = _STATE_LIMITS.get(k, (0, 100))
            try:
                num = float(v)
            except (TypeError, ValueError):
                continue
            valid[k] = int(max(lo, min(hi, num)))
    if dtype == "thermostat" and "mode" in patch:
        if patch["mode"] in _THERMOSTAT_MODES:
            valid["mode"] = patch["mode"]
    return bool(valid), valid, "" if valid else "no valid state keys for this device type"


def register_adapter(adapter):
    global _ADAPTER
    with _lock:
        _ADAPTER = adapter


def get_house_state() -> dict:
    with _lock:
        if _ADAPTER is not None:
            return _ADAPTER.get_house_state()
        return _load_state()


def get_device(device_id: str) -> dict | None:
    with _lock:
        state = _load_state()
        dev = state.get("devices", {}).get(device_id)
        if dev is None:
            return None
        return {"id": device_id, **dev}


def set_device(device_id: str, patch: dict) -> dict:
    with _lock:
        state = _load_state()
        devices = state.get("devices", {})
        if device_id not in devices:
            return {"ok": False, "error": f"Unknown device {device_id}"}
        dev = devices[device_id]
        ok, valid, err = _validate_patch(dev.get("type", ""), patch)
        if not ok:
            return {"ok": False, "error": err}
        dev.setdefault("state", {}).update(valid)
        state["updatedAt"] = datetime.now().isoformat()
        _save_state(state)
        return {"ok": True, "device": {"id": device_id, **dev}}


def set_room_state(room_id: str, patch: dict) -> dict:
    with _lock:
        state = _load_state()
        applied, skipped = [], []
        for dev_id, dev in state.get("devices", {}).items():
            if dev.get("room") != room_id:
                continue
            ok, valid, err = _validate_patch(dev.get("type", ""), patch)
            if ok:
                dev.setdefault("state", {}).update(valid)
                applied.append(dev_id)
            else:
                skipped.append(dev_id)
        if not applied and not skipped:
            return {"error": f"Room {room_id} not found"}
        state["updatedAt"] = datetime.now().isoformat()
        _save_state(state)
        return {"applied": applied, "skipped": skipped}


def run_scene(scene_id: str) -> dict:
    with _lock:
        state = _load_state()
        scene = state.get("scenes", {}).get(scene_id)
        if scene is None:
            return {"ok": False, "error": f"Scene {scene_id} not found"}
        applied, skipped = [], []
        for dev_id, patch in scene.get("actions", {}).items():
            dev = state.get("devices", {}).get(dev_id)
            if dev is None:
                skipped.append(dev_id)
                continue
            ok, valid, err = _validate_patch(dev.get("type", ""), patch)
            if ok:
                dev.setdefault("state", {}).update(valid)
                applied.append(dev_id)
            else:
                skipped.append(dev_id)
        state["updatedAt"] = datetime.now().isoformat()
        _save_state(state)
        return {"ok": True, "scene": scene, "applied": applied, "skipped": skipped}


def get_device_history(device_id: str, hours: int = 24) -> dict:
    state = _load_state()
    hist = state.get("history", {}).get(device_id, [])
    cutoff = time.time() - (hours or 24) * 3600
    return {
        "device_id": device_id,
        "hours": hours,
        "points": [h for h in hist if h.get("ts", 0) >= cutoff] if hist else [
            {"ts": int(time.time()) - i * 3600, "kwh": round(0.12 * (i % 7), 2)} for i in range(hours or 24)
        ],
    }


def reset_house_state() -> dict:
    with _lock:
        _save_state(_default_state())
        return {"ok": True, "state": _load_state()}


def house_summary() -> str:
    state = _load_state()
    lights_on = sum(1 for d in state.get("devices", {}).values() if d.get("type") == "light" and d.get("state", {}).get("on"))
    unlocked = sum(1 for d in state.get("devices", {}).values() if d.get("type") == "lock" and not d.get("state", {}).get("locked"))
    thermo = next((d for d in state.get("devices", {}).values() if d.get("type") == "thermostat"), None)
    energy = state.get("energy", {})
    return (
        f"House summary: {lights_on} lights on, {unlocked} locks unlocked, "
        f"thermostat {thermo.get('state', {}).get('mode', 'unknown')} at "
        f"{thermo.get('state', {}).get('target_temperature', '?')}°F, "
        f"today {energy.get('today_kwh', 0)} kWh."
    )
