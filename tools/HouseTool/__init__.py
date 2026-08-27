"""HouseTool — home-control tools.

Re-exports the implementations from ``house_tools`` so callers import them
from the category package: ``from tools.HouseTool import house_set_device``.
"""
from .house_tools import (
    house_get_state,
    house_get_device,
    house_set_device,
    house_run_scene,
    house_get_history,
)

__all__ = [
    "house_get_state",
    "house_get_device",
    "house_set_device",
    "house_run_scene",
    "house_get_history",
]
