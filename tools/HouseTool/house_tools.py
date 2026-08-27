"""HouseTool — home-control tools for the House agent.

Thin wrappers over ``house_manager`` (the single source of truth for the
mock smart home). Tools follow the string-result convention shared by the
rest of the registry. When a real backend is later registered as the
adapter, these signatures and outputs stay stable.
"""
import json
import os
import sys

_DESKTOP = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))), "Desktop")
if _DESKTOP not in sys.path:
    sys.path.insert(0, _DESKTOP)

import house_manager as hm


def _fmt_device(d: dict) -> str:
    state = ", ".join(f"{k}={v}" for k, v in d.get("state", {}).items())
    return f"{d.get('id')} [{d.get('type')}] {d.get('name')} ({d.get('roomId')}): {state}"


def house_get_state() -> str:
    """Read the full house state: rooms, devices, scenes, energy, security."""
    state = hm.get_house_state()
    lines = [f"## House ({len(state.get('rooms', []))} rooms, "
             f"{len(state.get('devices', []))} devices)"]
    lines.append(hm.house_summary())
    by_room: dict[str, list[dict]] = {}
    for d in state.get("devices", []):
        by_room.setdefault(d.get("roomId", "?"), []).append(d)
    for room in state.get("rooms", []):
        lines.append(f"\n### {room.get('name')} ({room.get('id')})")
        for d in by_room.get(room.get("id", ""), []):
            lines.append(f"- {_fmt_device(d)}")
    lines.append("\n### Scenes")
    for s in state.get("scenes", []):
        lines.append(f"- {s.get('id')} \"{s.get('name')}\" ({len(s.get('actions', []))} actions)")
    return "\n".join(lines)


def house_get_device(device_id: str) -> str:
    """Read one device's full record (state, capabilities, room, online)."""
    if not device_id:
        return "Error: device_id is required"
    d = hm.get_device(device_id)
    if d is None:
        return f"Error: Unknown device '{device_id}'"
    return _fmt_device(d) + f" | online={d.get('isOnline')} | writable={', '.join(d.get('capabilities', [])) or 'read-only'}"


def house_set_device(device_id: str, state: dict) -> str:
    """Write a state patch to one device (e.g. brightness=50, power=true)."""
    if not device_id:
        return "Error: device_id is required"
    if not isinstance(state, dict) or not state:
        return "Error: state must be a non-empty object like {\"brightness\": 50}"
    result = hm.set_device(device_id, state)
    if not result.get("ok"):
        return f"Error: {result.get('error')}"
    return f"[OK] {_fmt_device(result.get('device') or {})}"


def house_run_scene(scene_id: str) -> str:
    """Activate a scene (applies every action in it)."""
    if not scene_id:
        return "Error: scene_id is required"
    result = hm.run_scene(scene_id)
    if not result.get("ok"):
        return f"Error: {result.get('error')}"
    scene = result.get("scene") or {}
    skipped = result.get("skipped") or []
    out = f"[OK] Scene '{scene.get('name')}' ({scene_id}) applied to {len(result.get('applied', []))} device(s)"
    if skipped:
        out += f" | skipped: {', '.join(f'{sid} ({why})' for sid, why in skipped)}"
    return out


def house_get_history(device_id: str = "", hours: int = 24) -> str:
    """Mock usage history for one device (kWh series, up to 168h)."""
    if not device_id:
        return "Error: device_id is required"
    try:
        hours = int(hours)
    except (TypeError, ValueError):
        hours = 24
    result = hm.get_device_history(device_id, hours)
    if result.get("error"):
        return f"Error: {result['error']}"
    series = result.get("series", [])
    total = round(sum(series), 3)
    return (f"[OK] {device_id} last {result.get('hours')}h: total {total} kWh\n"
            f"series: {json.dumps(series)}")
