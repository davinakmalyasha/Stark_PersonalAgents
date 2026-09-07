"""
SHIM — read-only view over server.store.mode_store (Lane A hybrid).

User authority: plan/build mode is decided ONLY by the user via the prompt
dock (POST /api/chat/{id}/mode -> server.store.mode_store.set_mode).
Agents must never flip the mode — so this module exposes get_mode ONLY.
Any agent-side import of set_mode/clear_mode will fail by design.
Keeps VALID_MODES/DEFAULT_MODE for tool guard imports.
"""
from server.store.mode_store import get_mode
from typing import Literal
Mode = Literal["plan", "build"]
VALID_MODES: tuple[Mode, ...] = ("plan", "build")
DEFAULT_MODE: Mode = "build"
__all__ = ["Mode", "VALID_MODES", "DEFAULT_MODE", "get_mode"]
