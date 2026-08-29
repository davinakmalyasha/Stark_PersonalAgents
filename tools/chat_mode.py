"""
SHIM — use server.store.mode_store instead (Lane A hybrid).
Keeps VALID_MODES/DEFAULT_MODE for tool guard imports.
"""
from server.store.mode_store import get_mode, set_mode
from typing import Literal
Mode = Literal["plan", "build"]
VALID_MODES: tuple[Mode, ...] = ("plan", "build")
DEFAULT_MODE: Mode = "build"
def clear_mode(chat_id: str) -> None:
    try:
        from server.store.mode_store import set_mode as _set
        # clearing = reset to build
        if chat_id:
            _set(chat_id, "build")
    except Exception:
        pass
__all__ = ["Mode","VALID_MODES","DEFAULT_MODE","get_mode","set_mode","clear_mode"]
