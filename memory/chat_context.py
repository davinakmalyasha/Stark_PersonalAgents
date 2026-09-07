"""SHIM — use server.context.chat_history instead (Lane A)."""
from server.context.chat_history import build_chat_context
__all__ = ["build_chat_context"]
