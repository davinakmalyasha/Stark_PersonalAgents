"""
Sync Package
Provides live state synchronization, task tracking helpers, and LangGraph workflow orchestration.
"""
import os
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

_DESKTOP = os.path.join(_ROOT, "Desktop")
if os.path.isdir(_DESKTOP) and _DESKTOP not in sys.path:
    sys.path.insert(0, _DESKTOP)

_VOICE = os.path.join(_ROOT, "Voice")
if os.path.isdir(_VOICE) and _VOICE not in sys.path:
    sys.path.insert(0, _VOICE)

from .state_sync import (
    shared_state,
    state_lock,
    safe_get_state,
    safe_update_state,
    safe_append_live_log,
    safe_serialize_state,
    reset_shared_state,
)
from .sync_helpers import (
    load_task_tracking,
    save_task_tracking,
    plan_state,
    load_plan,
    save_plan,
    build_plan_progress_block,
)
from .sync_manager import (
    run_and_sync_graph,
)

__all__ = [
    "shared_state",
    "state_lock",
    "safe_get_state",
    "safe_update_state",
    "safe_append_live_log",
    "safe_serialize_state",
    "reset_shared_state",
    "load_task_tracking",
    "save_task_tracking",
    "plan_state",
    "load_plan",
    "save_plan",
    "build_plan_progress_block",
    "run_and_sync_graph",
]
