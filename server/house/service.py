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
    "light": ["power", "on", "brightness", "colorTemp"],
    "thermostat": ["mode", "targetTemp", "target_temperature"],
    "lock": ["locked"],
    "plug": ["power", "on"],
    "blinds": ["position"],
    "camera": ["recording"],
    "sensor": ["alert", "motion"],
}

_BOOL_KEYS = {"on", "power", "locked", "recording", "alert", "motion"}

# Legacy key → canonical key (the workspace UI writes the canonical form).
_KEY_ALIASES = {
    "on": "power",
    "target_temperature": "targetTemp",
}

_STATE_LIMITS = {
    "brightness": (0, 100),
    "target_temperature": (50, 90),
    "targetTemp": (10, 35),
    "position": (0, 100),
    "colorTemp": (1500, 9000),
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
        if k in _BOOL_KEYS:
            valid[_KEY_ALIASES.get(k, k)] = bool(v)
        else:
            lo, hi = _STATE_LIMITS.get(k, (0, 100))
            try:
                num = float(v)
            except (TypeError, ValueError):
                continue
            valid[_KEY_ALIASES.get(k, k)] = int(max(lo, min(hi, num)))
    if dtype == "thermostat" and "mode" in patch:
        if patch["mode"] in _THERMOSTAT_MODES:
            valid["mode"] = patch["mode"]
    return bool(valid), valid, "" if valid else "no valid state keys for this device type"


# ── Schema tolerance ─────────────────────────────────────────────────────────
# house_state.json exists in two shapes: the legacy dict-keyed form
# ({"devices": {id: {...}}, "scenes": {id: {"actions": {dev: patch}}}}) and the
# list form written by the workspace UI
# ({"devices": [{id, roomId, state: {power, ...}, ...}],
#   "scenes": [{id, actions: [{deviceId, state}]}]}).
# All access normalizes through these helpers; writes mutate the loaded
# structure in place so the on-disk shape is never migrated implicitly.


def _devices_map(state: dict) -> dict:
    """{device_id: device} for either on-disk shape (never raises)."""
    devices = (state or {}).get("devices", {})
    if isinstance(devices, dict):
        return devices
    out: dict = {}
    if isinstance(devices, list):
        for dev in devices:
            if isinstance(dev, dict) and dev.get("id"):
                out[dev["id"]] = dev
    return out


def _scenes_map(state: dict) -> dict:
    """{scene_id: scene} for either on-disk shape (never raises)."""
    scenes = (state or {}).get("scenes", {})
    if isinstance(scenes, dict):
        return scenes
    out: dict = {}
    if isinstance(scenes, list):
        for sc in scenes:
            if isinstance(sc, dict) and sc.get("id"):
                out[sc["id"]] = sc
    return out


def _scene_actions(scene: dict) -> list[tuple]:
    """Normalize scene actions to [(device_id, patch)] for both shapes."""
    actions = (scene or {}).get("actions", [])
    if isinstance(actions, dict):
        return [(dev_id, patch) for dev_id, patch in actions.items()
                if isinstance(patch, dict)]
    out: list[tuple] = []
    if isinstance(actions, list):
        for a in actions:
            if not isinstance(a, dict):
                continue
            dev_id = a.get("deviceId") or a.get("device_id") or a.get("dev") or a.get("id")
            patch = a.get("state") if isinstance(a.get("state"), dict) else a.get("patch")
            if dev_id and isinstance(patch, dict):
                out.append((dev_id, patch))
    return out


def _dev_room(dev: dict) -> str:
    return (dev or {}).get("room") or (dev or {}).get("roomId") or ""


def _dev_state(dev: dict | None) -> dict:
    st = (dev or {}).get("state")
    return st if isinstance(st, dict) else {}


def _dev_power_on(dev: dict) -> bool:
    st = _dev_state(dev)
    if "power" in st:
        return bool(st.get("power"))
    return bool(st.get("on"))


def _dev_target_temp(dev: dict):
    st = _dev_state(dev)
    return st.get("targetTemp", st.get("target_temperature", "?"))


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
        dev = _devices_map(state).get(device_id)
        if dev is None:
            return None
        return {"id": device_id, **dev}


def set_device(device_id: str, patch: dict) -> dict:
    with _lock:
        state = _load_state()
        dev = _devices_map(state).get(device_id)
        if dev is None:
            return {"ok": False, "error": f"Unknown device {device_id}"}
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
        for dev_id, dev in _devices_map(state).items():
            if _dev_room(dev) != room_id:
                continue
            ok, valid, err = _validate_patch(dev.get("type", ""), patch)
            if ok:
                dev.setdefault("state", {}).update(valid)
                applied.append(dev_id)
            else:
                skipped.append([dev_id, err or "unsupported keys"])
        if not applied and not skipped:
            return {"error": f"Room {room_id} not found"}
        state["updatedAt"] = datetime.now().isoformat()
        _save_state(state)
        return {"applied": applied, "skipped": skipped}


def run_scene(scene_id: str) -> dict:
    with _lock:
        state = _load_state()
        scene = _scenes_map(state).get(scene_id)
        if scene is None:
            return {"ok": False, "error": f"Scene {scene_id} not found"}
        devices = _devices_map(state)
        applied, skipped = [], []
        for dev_id, patch in _scene_actions(scene):
            dev = devices.get(dev_id)
            if dev is None:
                skipped.append([dev_id, "unknown device"])
                continue
            ok, valid, err = _validate_patch(dev.get("type", ""), patch)
            if ok:
                dev.setdefault("state", {}).update(valid)
                applied.append(dev_id)
            else:
                skipped.append([dev_id, err or "unsupported keys"])
        state["updatedAt"] = datetime.now().isoformat()
        _save_state(state)
        return {"ok": True, "scene": scene, "applied": applied, "skipped": skipped}


def get_device_history(device_id: str, hours: int = 24) -> dict:
    state = _load_state()
    hist = state.get("history", {}).get(device_id, []) if isinstance(state.get("history"), dict) else []
    cutoff = time.time() - (hours or 24) * 3600
    points = [h for h in hist if isinstance(h, dict) and h.get("ts", 0) >= cutoff] if hist else [
        {"ts": int(time.time()) - i * 3600, "kwh": round(0.12 * (i % 7), 2)} for i in range(hours or 24)
    ]
    return {
        "device_id": device_id,
        "hours": hours,
        "points": points,
        "series": [p.get("kwh", 0) for p in points if isinstance(p, dict)],
    }


def reset_house_state() -> dict:
    with _lock:
        _save_state(_default_state())
        return {"ok": True, "state": _load_state()}


def house_summary() -> str:
    state = _load_state()
    devices = list(_devices_map(state).values())
    lights_on = sum(1 for d in devices if d.get("type") == "light" and _dev_power_on(d))
    unlocked = sum(1 for d in devices if d.get("type") == "lock" and not _dev_state(d).get("locked"))
    thermo = next((d for d in devices if d.get("type") == "thermostat"), None)
    tstate = _dev_state(thermo)
    energy = state.get("energy", {}) or {}
    temp = tstate.get("targetTemp", tstate.get("target_temperature", "?"))
    return (
        f"House summary: {lights_on} lights on, {unlocked} locks unlocked, "
        f"thermostat {tstate.get('mode', 'unknown')} at "
        f"{temp}°, "
        f"today {energy.get('today_kwh', 0)} kWh."
    )
