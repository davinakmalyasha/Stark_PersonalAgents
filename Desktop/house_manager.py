"""SHIM — use server.house.service instead (Phase 7)."""
from server.house.service import (
    register_adapter, get_house_state, get_device, set_device,
    set_room_state, run_scene, get_device_history, reset_house_state,
    house_summary,
)
