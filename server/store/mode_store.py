"""
Per-chat plan|build mode — persisted per chat (survives restart).
Migrated from tools/chat_mode.py in-memory to SQLite chats.mode column + file fallback.
"""
import os
import json
import threading

MODE_PATH = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chat_modes.json")
_lock = threading.RLock()

def _load_modes() -> dict:
    if not os.path.isfile(MODE_PATH):
        return {}
    try:
        with open(MODE_PATH, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return {}

def _save_modes(data: dict) -> None:
    try:
        os.makedirs(os.path.dirname(MODE_PATH), exist_ok=True)
        with open(MODE_PATH, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2)
    except Exception:
        pass

def get_mode(chat_id: str) -> str:
    """Return 'plan' or 'build' (default build). Checks SQLite chats.mode first, then file."""
    if not chat_id:
        return "build"
    # Try SQLite
    try:
        from server.store.chat_store import _get_db
        db = _get_db()
        cur = db.execute("SELECT mode FROM chats WHERE id=?", (chat_id,))
        row = cur.fetchone()
        if row and row[0] in ("plan", "build"):
            return row[0]
    except Exception:
        pass
    # Fallback file
    with _lock:
        modes = _load_modes()
        return modes.get(chat_id, "build")

def set_mode(chat_id: str, mode: str) -> bool:
    if not chat_id or mode not in ("plan", "build"):
        return False
    # Persist to SQLite if chat exists
    try:
        from server.store.chat_store import _get_db, _ensure_dir
        db = _get_db()
        cur = db.execute("SELECT id FROM chats WHERE id=?", (chat_id,))
        if cur.fetchone():
            db.execute("UPDATE chats SET mode=? WHERE id=?", (mode, chat_id))
            db.commit()
        else:
            # Fallback file for chats not yet in SQLite (legacy)
            with _lock:
                modes = _load_modes()
                modes[chat_id] = mode
                _save_modes(modes)
            return True
        # Also mirror to file for backward compat
        with _lock:
            modes = _load_modes()
            modes[chat_id] = mode
            _save_modes(modes)
        return True
    except Exception:
        with _lock:
            modes = _load_modes()
            modes[chat_id] = mode
            _save_modes(modes)
        return True
