"""
House Manager — single source of truth for the House Workspace state.

The House is currently a MOCK smart home: state lives in one JSON file
(``.deep_agents/house_state.json``) and every mutation goes through this
module, so the UI, the House agent tools, and any future real integration
all observe and actuate the same state.

Adapter seam: a real backend (Home Assistant REST, MQTT, Hue, ...) can be
plugged in later by registering an adapter object exposing
``get_state()`` / ``set_device(device_id, patch)`` / ``run_scene(scene_id)``.
When an adapter is registered it replaces the file-backed mock; nothing
else in the system changes.
"""
import os
import json
import threading
from datetime import datetime

_ROOT = r"d:\MyProject\LangChain"
HOUSE_STATE_PATH = os.path.join(_ROOT, ".deep_agents", "house_state.json")

_LOCK = threading.RLock()
_ADAPTER = None

# Capability contract per device type: the only state keys an actor
# (UI control or agent tool) may write. Sensors are read-only; their
# values drift on each read so the dashboard feels alive.
_DEVICE_CAPABILITIES = {
    "light": ["power", "brightness", "colorTemp"],
    "thermostat": ["targetTemp", "mode"],
    "lock": ["locked"],
    "plug": ["power"],
    "blinds": ["position"],
    "camera": ["recording"],
    "sensor": [],
}

_STATE_LIMITS = {
    "brightness": (0, 100),
    "colorTemp": (2200, 6500),
    "position": (0, 100),
    "targetTemp": (10, 35),
}

_THERMOSTAT_MODES = {"off", "heat", "cool", "auto"}


# ── Seed state (mock house) ──────────────────────────────────────────────────


def _dev(device_id: str, name: str, room: str, dtype: str, state: dict, now: str) -> dict:
    return {
        "id": device_id,
        "name": name,
        "roomId": room,
        "type": dtype,
        "state": state,
        "capabilities": list(_DEVICE_CAPABILITIES[dtype]),
        "isOnline": True,
        "updatedAt": now,
    }


def _seed_devices(now: str) -> list[dict]:
    lights = {
        "light-living-1": ("Ceiling Light", "living", True, 70, 3500),
        "light-living-2": ("Floor Lamp", "living", False, 40, 2700),
        "light-kitchen-1": ("Main Light", "kitchen", True, 85, 4000),
        "light-bedroom-1": ("Bedside Lamp", "bedroom", False, 25, 2700),
        "light-bedroom-2": ("Ceiling Light", "bedroom", False, 60, 3000),
        "light-bathroom": ("Mirror Light", "bathroom", False, 90, 4000),
        "light-garage": ("Garage Light", "garage", False, 100, 5000),
    }
    plugs = {
        "plug-living-tv": ("Smart TV", "living", False),
        "plug-kitchen-coffee": ("Coffee Machine", "kitchen", False),
        "fan-bathroom": ("Exhaust Fan", "bathroom", False),
        "plug-garage-ev": ("EV Charger", "garage", False),
    }
    devices = [
        _dev(did, name, room, "light",
             {"power": power, "brightness": bright, "colorTemp": ctemp}, now)
        for did, (name, room, power, bright, ctemp) in lights.items()
    ]
    devices += [
        _dev(did, name, room, "plug", {"power": power}, now)
        for did, (name, room, power) in plugs.items()
    ]
    devices += [
        _dev("blind-living", "Window Blinds", "living", "blinds", {"position": 60}, now),
        _dev("blind-bedroom", "Bedroom Blinds", "bedroom", "blinds", {"position": 0}, now),
        _dev("sensor-living", "Climate Sensor", "living", "sensor",
             {"temperature": 23.5, "humidity": 45, "motion": False}, now),
        _dev("sensor-kitchen", "Smoke Sensor", "kitchen", "sensor",
             {"smoke": False, "temperature": 24.0}, now),
        _dev("sensor-bathroom", "Humidity Sensor", "bathroom", "sensor",
             {"humidity": 55, "temperature": 22.0}, now),
        _dev("thermostat-bedroom", "Thermostat", "bedroom", "thermostat",
             {"targetTemp": 22.0, "currentTemp": 22.5, "mode": "auto"}, now),
        _dev("lock-bedroom", "Balcony Door Lock", "bedroom", "lock", {"locked": True}, now),
        _dev("lock-garage", "Garage Door", "garage", "lock", {"locked": True}, now),
        _dev("cam-living", "Living Camera", "living", "camera", {"recording": True}, now),
        _dev("cam-garage", "Garage Camera", "garage", "camera", {"recording": True}, now),
    ]
    return devices


def _seed_scenes() -> list[dict]:
    return [
        {
            "id": "scene-morning", "name": "Good Morning", "icon": "sunrise",
            "actions": [
                {"deviceId": "light-living-1", "state": {"power": True, "brightness": 80}},
                {"deviceId": "blind-living", "state": {"position": 100}},
                {"deviceId": "blind-bedroom", "state": {"position": 100}},
                {"deviceId": "thermostat-bedroom", "state": {"targetTemp": 22.0}},
                {"deviceId": "plug-kitchen-coffee", "state": {"power": True}},
            ],
        },
        {
            "id": "scene-night", "name": "Good Night", "icon": "moon",
            "actions": [
                {"deviceId": "light-living-1", "state": {"power": False}},
                {"deviceId": "light-living-2", "state": {"power": False}},
                {"deviceId": "light-kitchen-1", "state": {"power": False}},
                {"deviceId": "light-bedroom-1", "state": {"power": False}},
                {"deviceId": "light-bedroom-2", "state": {"power": False}},
                {"deviceId": "blind-living", "state": {"position": 0}},
                {"deviceId": "blind-bedroom", "state": {"position": 0}},
                {"deviceId": "thermostat-bedroom", "state": {"targetTemp": 20.0}},
                {"deviceId": "lock-bedroom", "state": {"locked": True}},
                {"deviceId": "lock-garage", "state": {"locked": True}},
            ],
        },
        {
            "id": "scene-away", "name": "Away", "icon": "shield",
            "actions": [
                {"deviceId": "light-living-1", "state": {"power": False}},
                {"deviceId": "light-living-2", "state": {"power": False}},
                {"deviceId": "light-kitchen-1", "state": {"power": False}},
                {"deviceId": "light-bedroom-1", "state": {"power": False}},
                {"deviceId": "light-bedroom-2", "state": {"power": False}},
                {"deviceId": "light-bathroom", "state": {"power": False}},
                {"deviceId": "light-garage", "state": {"power": False}},
                {"deviceId": "plug-living-tv", "state": {"power": False}},
                {"deviceId": "plug-kitchen-coffee", "state": {"power": False}},
                {"deviceId": "cam-living", "state": {"recording": True}},
                {"deviceId": "cam-garage", "state": {"recording": True}},
                {"deviceId": "lock-bedroom", "state": {"locked": True}},
                {"deviceId": "lock-garage", "state": {"locked": True}},
            ],
        },
        {
            "id": "scene-movie", "name": "Movie Night", "icon": "tv",
            "actions": [
                {"deviceId": "light-living-1", "state": {"power": True, "brightness": 15, "colorTemp": 2700}},
                {"deviceId": "light-living-2", "state": {"power": True, "brightness": 10}},
                {"deviceId": "blind-living", "state": {"position": 0}},
                {"deviceId": "plug-living-tv", "state": {"power": True}},
            ],
        },
        {
            "id": "scene-focus", "name": "Focus", "icon": "target",
            "actions": [
                {"deviceId": "light-living-1", "state": {"power": True, "brightness": 100, "colorTemp": 5000}},
                {"deviceId": "blind-living", "state": {"position": 100}},
                {"deviceId": "thermostat-bedroom", "state": {"targetTemp": 21.0}},
            ],
        },
    ]


def _seed_state() -> dict:
    now = datetime.now().isoformat()
    return {
        "version": 1,
        "updatedAt": now,
        "rooms": [
            {"id": "living", "name": "Living Room", "icon": "sofa"},
            {"id": "kitchen", "name": "Kitchen", "icon": "utensils"},
            {"id": "bedroom", "name": "Bedroom", "icon": "bed"},
            {"id": "bathroom", "name": "Bathroom", "icon": "bath"},
            {"id": "garage", "name": "Garage", "icon": "car"},
        ],
        "devices": _seed_devices(now),
        "scenes": _seed_scenes(),
        "energy": {
            "todayKwh": 12.4,
            "cost": 2.86,
            "series": [0.2, 0.2, 0.2, 0.2, 0.2, 0.3, 0.6, 0.9, 0.8, 0.7,
                       0.6, 0.6, 0.7, 0.6, 0.6, 0.7, 0.9, 1.2, 1.5, 1.4,
                       1.1, 0.8, 0.5, 0.3],
        },
        "security": {
            "armed": False,
            "mode": "home",
            "lastEvent": "System disarmed",
        },
    }


# ── Persistence ──────────────────────────────────────────────────────────────


def _load_state() -> dict:
    """Load house state from disk, seeding it on first access."""
    with _LOCK:
        os.makedirs(os.path.dirname(HOUSE_STATE_PATH), exist_ok=True)
        if os.path.isfile(HOUSE_STATE_PATH):
            try:
                with open(HOUSE_STATE_PATH, "r", encoding="utf-8") as f:
                    data = json.load(f)
                if isinstance(data, dict) and data.get("devices"):
                    return data
            except Exception as e:
                print(f"[HOUSE] State load failed, reseeding: {e}")
        data = _seed_state()
        _save_state(data)
        return data


def _save_state(data: dict) -> None:
    data["updatedAt"] = datetime.now().isoformat()
    with open(HOUSE_STATE_PATH, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)


# ── Validation ───────────────────────────────────────────────────────────────


def _validate_patch(dtype: str, patch: dict) -> tuple[dict, str]:
    """Filter a state patch to writable keys for this device type.

    Returns ``(clean_patch, error)`` — error is empty on success.
    """
    allowed = _DEVICE_CAPABILITIES.get(dtype)
    if allowed is None:
        return {}, f"Unknown device type '{dtype}'"
    if not allowed:
        return {}, "Sensors are read-only"
    if not isinstance(patch, dict) or not patch:
        return {}, "Empty state patch"
    clean: dict = {}
    for key, value in patch.items():
        if key not in allowed:
            return {}, f"'{key}' is not writable on a {dtype} (allowed: {', '.join(allowed)})"
        if key in _STATE_LIMITS:
            low, high = _STATE_LIMITS[key]
            try:
                value = float(value)
            except (TypeError, ValueError):
                return {}, f"'{key}' must be a number"
            value = max(low, min(high, value))
            if key == "targetTemp":
                value = round(value, 1)
            else:
                value = int(round(value))
        if key == "power":
            value = bool(value)
        if key == "locked":
            value = bool(value)
        if key == "mode":
            value = str(value).lower()
            if value not in _THERMOSTAT_MODES:
                return {}, f"mode must be one of: {', '.join(sorted(_THERMOSTAT_MODES))}"
        if key == "recording":
            value = bool(value)
        clean[key] = value
    return clean, ""


# ── Public API ───────────────────────────────────────────────────────────────


def register_adapter(adapter) -> None:
    """Plug in a real backend. The adapter must expose ``get_state()``,
    ``set_device(device_id, patch)`` and ``run_scene(scene_id)``. Passing
    ``None`` reverts to the file-backed mock."""
    global _ADAPTER
    _ADAPTER = adapter


def get_house_state() -> dict:
    if _ADAPTER is not None:
        return _ADAPTER.get_state()
    with _LOCK:
        return _load_state()


def get_device(device_id: str) -> dict | None:
    state = get_house_state()
    return next((d for d in state.get("devices", []) if d.get("id") == device_id), None)


def set_device(device_id: str, patch: dict) -> dict:
    """Apply a validated state patch to one device.

    Returns ``{"ok": bool, "device": {...} | None, "error": str}``.
    """
    if _ADAPTER is not None:
        return _ADAPTER.set_device(device_id, patch)
    with _LOCK:
        data = _load_state()
        device = next((d for d in data.get("devices", []) if d.get("id") == device_id), None)
        if device is None:
            return {"ok": False, "device": None, "error": f"Unknown device '{device_id}'"}
        if not device.get("isOnline", True):
            return {"ok": False, "device": device, "error": f"Device '{device_id}' is offline"}
        clean, err = _validate_patch(device.get("type", ""), patch)
        if err:
            return {"ok": False, "device": device, "error": err}
        device["state"].update(clean)
        device["updatedAt"] = datetime.now().isoformat()
        _save_state(data)
        return {"ok": True, "device": device, "error": ""}


def set_room_state(room_id: str, patch: dict) -> dict:
    """Apply one patch to every device in a room that supports those keys.

    Returns ``{"applied": [device_ids], "skipped": [(id, reason)], "error": ""}``.
    """
    if _ADAPTER is not None and hasattr(_ADAPTER, "set_room_state"):
        return _ADAPTER.set_room_state(room_id, patch)
    with _LOCK:
        data = _load_state()
        room = next((r for r in data.get("rooms", []) if r.get("id") == room_id), None)
        if room is None:
            return {"applied": [], "skipped": [], "error": f"Unknown room '{room_id}'"}
        applied: list[str] = []
        skipped: list[tuple] = []
        for device in data.get("devices", []):
            if device.get("roomId") != room_id:
                continue
            clean, err = _validate_patch(device.get("type", ""), patch)
            if err:
                skipped.append((device["id"], err))
                continue
            device["state"].update(clean)
            device["updatedAt"] = datetime.now().isoformat()
            applied.append(device["id"])
        _save_state(data)
        return {"applied": applied, "skipped": skipped, "error": ""}


def run_scene(scene_id: str) -> dict:
    """Apply every action in a scene. Returns applied/skipped details."""
    if _ADAPTER is not None:
        return _ADAPTER.run_scene(scene_id)
    with _LOCK:
        data = _load_state()
        scene = next((s for s in data.get("scenes", []) if s.get("id") == scene_id), None)
        if scene is None:
            known = ", ".join(s.get("id", "") for s in data.get("scenes", []))
            return {"ok": False, "scene": None, "applied": [], "skipped": [],
                    "error": f"Unknown scene '{scene_id}' (available: {known})"}
        applied: list[str] = []
        skipped: list[tuple] = []
        by_id = {d["id"]: d for d in data.get("devices", [])}
        for action in scene.get("actions", []):
            device = by_id.get(action.get("deviceId", ""))
            if device is None:
                skipped.append((action.get("deviceId", ""), "device not found"))
                continue
            clean, err = _validate_patch(device.get("type", ""), action.get("state", {}))
            if err:
                skipped.append((device["id"], err))
                continue
            device["state"].update(clean)
            device["updatedAt"] = datetime.now().isoformat()
            applied.append(device["id"])
        _save_state(data)
        return {"ok": True, "scene": scene, "applied": applied, "skipped": skipped, "error": ""}


def get_device_history(device_id: str, hours: int = 24) -> dict:
    """Deterministic mock usage series for one device (stable per hour)."""
    device = get_device(device_id)
    if device is None:
        return {"error": f"Unknown device '{device_id}'", "series": []}
    import random
    rng = random.Random(f"{device_id}-{datetime.now().strftime('%Y%m%d')}")
    hours = max(1, min(hours, 168))
    base = 0.05 if device.get("type") in ("light", "plug") else 0.01
    series = [round(base + rng.random() * (0.4 if rng.random() > 0.7 else 0.15), 3)
              for _ in range(hours)]
    return {"device": device.get("id", ""), "hours": hours, "unit": "kWh", "series": series}


def reset_house_state() -> dict:
    """Wipe the mock state and reseed (test/demo helper)."""
    global _ADAPTER
    if _ADAPTER is not None:
        return {"ok": False, "error": "A real adapter is registered; mock reset unavailable"}
    with _LOCK:
        try:
            if os.path.isfile(HOUSE_STATE_PATH):
                os.remove(HOUSE_STATE_PATH)
        except Exception as e:
            return {"ok": False, "error": str(e)}
        data = _load_state()
        return {"ok": True, "rooms": len(data.get("rooms", [])),
                "devices": len(data.get("devices", []))}


def house_summary() -> str:
    """Compact one-glance summary of the house for agent context blocks."""
    state = get_house_state()
    devices = state.get("devices", [])
    lights_on = sum(1 for d in devices if d.get("type") == "light" and d["state"].get("power"))
    unlocked = [d.get("name", d.get("id")) for d in devices
                if d.get("type") == "lock" and not d["state"].get("locked")]
    thermo = next((d for d in devices if d.get("type") == "thermostat"), None)
    energy = state.get("energy", {})
    security = state.get("security", {})
    lines = [
        f"Rooms: {len(state.get('rooms', []))} | Devices: {len(devices)} "
        f"| Lights on: {lights_on} | Security: {'ARMED' if security.get('armed') else 'disarmed'}",
        f"Climate: {thermo['state'].get('currentTemp')}C (target {thermo['state'].get('targetTemp')}C)"
        if thermo else "Climate: no thermostat",
        f"Unlocked: {', '.join(unlocked) if unlocked else 'none'}"
        + f" | Energy today: {energy.get('todayKwh')} kWh (${energy.get('cost')})",
    ]
    return "\n".join(lines)
