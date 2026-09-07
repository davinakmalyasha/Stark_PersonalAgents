"""
Workspace Manager — SHIM for backward compat (Lane A).
Original God file split into server/store/* (hybrid JSONL+SQLite).
This shim re-exports so existing `import workspace_manager as wm` via sys.path Desktop keeps working.
New code should `from server.store import ...` directly.
"""
import os as _os
import sys as _sys
# Ensure server is importable when Desktop is on sys.path (api_server path hack)
_root = _os.path.dirname(_os.path.dirname(_os.path.abspath(__file__)))
if _root not in _sys.path:
    _sys.path.insert(0, _root)

from server.store.chat_store import (
    SESSIONS_ROOT,
    CHATS_DB,
    _ensure_dir,
    _chat_dir,
    list_chats,
    get_chat,
    create_chat,
    save_chat,
    delete_chat,
    add_message,
    update_chat_title,
    get_chat_tasks,
    save_chat_tasks,
)
from server.store.workspace_registry import (
    REGISTRY_PATH,
    GENERAL_WORKSPACE_ID,
    HOUSE_WORKSPACE_ID,
    PERSONAL_WORKSPACE_ID,
    VIRTUAL_WORKSPACE_IDS,
    GENERAL_CHAT_DIR,
    HOUSE_CHAT_DIR,
    PERSONAL_CHAT_DIR,
    list_workspaces,
    add_workspace,
    remove_workspace,
    get_workspace,
    cleanup_orphaned_chat_dirs,
    init_default_workspace,
    probe_environment,
    get_workspace_rules_and_profile,
    get_client_name,
)
from server.store.mode_store import get_mode

# Legacy aliases
__all__ = [
    "REGISTRY_PATH", "GENERAL_WORKSPACE_ID", "HOUSE_WORKSPACE_ID", "PERSONAL_WORKSPACE_ID",
    "VIRTUAL_WORKSPACE_IDS", "GENERAL_CHAT_DIR", "HOUSE_CHAT_DIR", "PERSONAL_CHAT_DIR",
    "list_workspaces", "add_workspace", "remove_workspace", "get_workspace",
    "cleanup_orphaned_chat_dirs", "init_default_workspace", "probe_environment",
    "get_workspace_rules_and_profile", "get_client_name",
    "list_chats", "create_chat", "get_chat", "save_chat", "delete_chat", "add_message",
    "update_chat_title", "get_chat_tasks", "save_chat_tasks",
    "get_mode",
]
