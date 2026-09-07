"""
Hybrid sharded JSONL + SQLite chat store — Lane A.

Canonical: .deep_agents/sessions/{kind}/{wsId}/{chatId}.jsonl (Pi-style append-only, one line per MessageEntry)
Index: .deep_agents/chats.db SQLite WAL (chats + messages + FTS5) for fast list/get/search + per-chat plan|build mode.

Migration: if SQLite empty but legacy .deep_agents/chats/{wsId}/{chatId}.json exists, lazily import on first get_chat/list_chats.
"""
import os
import json
import re
import uuid
import sqlite3
import threading
from datetime import datetime

# Paths
SESSIONS_ROOT = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "sessions")
CHATS_DB = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats.db")
LEGACY_CHATS_ROOT = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats")

# Virtual workspace helpers (import lazily to avoid circular)
GENERAL_WORKSPACE_ID = "_general"
HOUSE_WORKSPACE_ID = "_house"
PERSONAL_WORKSPACE_ID = "_personal"
DFD_WORKSPACE_ID = "_dfd"

_lock = threading.RLock()
_db: sqlite3.Connection | None = None
# Per-chat locks for parallel runs across workspaces (Phase 2 P0)
_CHAT_LOCKS: dict[str, threading.RLock] = {}
_CHAT_LOCKS_LOCK = threading.RLock()

def _chat_lock(chat_id: str) -> threading.RLock:
    with _CHAT_LOCKS_LOCK:
        lk = _CHAT_LOCKS.get(chat_id)
        if lk is None:
            lk = threading.RLock()
            _CHAT_LOCKS[chat_id] = lk
        return lk

def _ensure_dir(path: str) -> None:
    os.makedirs(path, exist_ok=True)

def _get_db() -> sqlite3.Connection:
    global _db
    with _lock:
        if _db is not None:
            return _db
        _ensure_dir(os.path.dirname(CHATS_DB))
        db = sqlite3.connect(CHATS_DB, check_same_thread=False, isolation_level=None)
        db.execute("PRAGMA journal_mode=WAL;")
        db.execute("PRAGMA synchronous=NORMAL;")
        _init_db(db)
        _db = db
        return _db

def _init_db(db: sqlite3.Connection) -> None:
    db.execute("""
        CREATE TABLE IF NOT EXISTS chats (
            id TEXT PRIMARY KEY,
            workspace_id TEXT,
            workspace_path TEXT,
            title TEXT,
            model TEXT,
            created_at TEXT,
            updated_at TEXT,
            message_count INTEGER DEFAULT 0,
            mode TEXT DEFAULT 'build',
            pinned INTEGER DEFAULT 0,
            tasks_json TEXT
        )
    """)
    db.execute("""
        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            chat_id TEXT,
            role TEXT,
            content TEXT,
            timestamp TEXT,
            is_trace INTEGER DEFAULT 0,
            metadata_json TEXT,
            FOREIGN KEY(chat_id) REFERENCES chats(id) ON DELETE CASCADE
        )
    """)
    db.execute("CREATE INDEX IF NOT EXISTS idx_messages_chat_ts ON messages(chat_id, timestamp)")
    db.execute("CREATE INDEX IF NOT EXISTS idx_chats_ws_updated ON chats(workspace_id, updated_at)")
    # Additive migration: tasks_json column for the light task scheduler path
    # (older DBs created before the column existed get it added in place).
    cols = [row[1] for row in db.execute("PRAGMA table_info(chats)").fetchall()]
    if "tasks_json" not in cols:
        db.execute("ALTER TABLE chats ADD COLUMN tasks_json TEXT")
        db.commit()
    # FTS5 for title/content search
    try:
        db.execute("""
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                content, title, content='messages', content_rowid='rowid', tokenize='porter'
            )
        """)
        # Triggers to keep FTS in sync
        db.execute("""
            CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, content, title) VALUES (new.rowid, new.content, '');
            END
        """)
        db.execute("""
            CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, content, title) VALUES('delete', old.rowid, old.content, '');
            END
        """)
        db.execute("""
            CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, content, title) VALUES('delete', old.rowid, old.content, '');
                INSERT INTO messages_fts(rowid, content, title) VALUES (new.rowid, new.content, '');
            END
        """)
    except Exception:
        pass

def _validate_chat_id(chat_id: str) -> None:
    if chat_id and not re.match(r'^[\w\-.]+$', chat_id):
        raise ValueError(f"Invalid chat_id: {chat_id}")

def _chat_dir(workspace_path: str) -> str:
    if workspace_path == GENERAL_WORKSPACE_ID or workspace_path == '':
        _ensure_dir(os.path.join(LEGACY_CHATS_ROOT, GENERAL_WORKSPACE_ID))
        return os.path.join(LEGACY_CHATS_ROOT, GENERAL_WORKSPACE_ID)
    if workspace_path == HOUSE_WORKSPACE_ID:
        _ensure_dir(os.path.join(LEGACY_CHATS_ROOT, HOUSE_WORKSPACE_ID))
        return os.path.join(LEGACY_CHATS_ROOT, HOUSE_WORKSPACE_ID)
    if workspace_path == PERSONAL_WORKSPACE_ID:
        _ensure_dir(os.path.join(LEGACY_CHATS_ROOT, PERSONAL_WORKSPACE_ID))
        return os.path.join(LEGACY_CHATS_ROOT, PERSONAL_WORKSPACE_ID)
    if workspace_path == DFD_WORKSPACE_ID:
        _ensure_dir(os.path.join(LEGACY_CHATS_ROOT, DFD_WORKSPACE_ID))
        return os.path.join(LEGACY_CHATS_ROOT, DFD_WORKSPACE_ID)
    workspace_path = os.path.normpath(os.path.abspath(workspace_path))
    # For project workspaces, find ws id via registry
    try:
        from server.store.workspace_registry import _load_registry
        reg = _load_registry()
        for ws in reg.get("workspaces", []):
            if os.path.normpath(ws.get("path", "")).lower() == workspace_path.lower():
                ws_id = ws.get("id")
                central_dir = os.path.join(LEGACY_CHATS_ROOT, ws_id)
                _ensure_dir(central_dir)
                path_file = os.path.join(central_dir, "workspace_path.txt")
                if not os.path.isfile(path_file):
                    try:
                        with open(path_file, "w", encoding="utf-8") as f:
                            f.write(workspace_path)
                    except Exception:
                        pass
                return central_dir
    except Exception:
        pass
    fallback_dir = os.path.join(LEGACY_CHATS_ROOT, "_unregistered")
    _ensure_dir(fallback_dir)
    return fallback_dir

def _jsonl_dir(workspace_path: str) -> str:
    """Sharded JSONL dir: sessions/{kind}/{wsId}"""
    if workspace_path in (GENERAL_WORKSPACE_ID, ''):
        kind = "general"
        ws_id = GENERAL_WORKSPACE_ID
    elif workspace_path == HOUSE_WORKSPACE_ID:
        kind = "house"
        ws_id = HOUSE_WORKSPACE_ID
    elif workspace_path == PERSONAL_WORKSPACE_ID:
        kind = "personal"
        ws_id = PERSONAL_WORKSPACE_ID
    elif workspace_path == DFD_WORKSPACE_ID:
        kind = "dfd"
        ws_id = DFD_WORKSPACE_ID
    else:
        kind = "project"
        # Resolve ws id
        ws_id = "_unregistered"
        try:
            from server.store.workspace_registry import _load_registry
            reg = _load_registry()
            norm = os.path.normpath(workspace_path).lower()
            for ws in reg.get("workspaces", []):
                if os.path.normpath(ws.get("path", "")).lower() == norm:
                    ws_id = ws.get("id")
                    break
        except Exception:
            pass
    d = os.path.join(SESSIONS_ROOT, kind, ws_id)
    _ensure_dir(d)
    return d

def _jsonl_path(workspace_path: str, chat_id: str) -> str:
    return os.path.join(_jsonl_dir(workspace_path), f"{chat_id}.jsonl")

# Public API — mirrors old workspace_manager chat CRUD but via SQLite+JSONL

def list_chats(workspace_path: str) -> list[dict]:
    db = _get_db()
    # Determine workspace_id for filter
    ws_id = workspace_path or GENERAL_WORKSPACE_ID
    if workspace_path not in (GENERAL_WORKSPACE_ID, HOUSE_WORKSPACE_ID, PERSONAL_WORKSPACE_ID, DFD_WORKSPACE_ID, '', None):
        # Resolve to real ws id
        try:
            from server.store.workspace_registry import _load_registry
            reg = _load_registry()
            norm = os.path.normpath(workspace_path).lower()
            for ws in reg.get("workspaces", []):
                if os.path.normpath(ws.get("path", "")).lower() == norm:
                    ws_id = ws.get("id")
                    break
        except Exception:
            pass
    # Lazy migration removed — chats.db + sessions/{kind}/ JSONL are canonical
    try:
        cur = db.execute("SELECT id, title, model, created_at, updated_at, message_count FROM chats WHERE workspace_id=? ORDER BY updated_at DESC", (ws_id,))
        rows = cur.fetchall()
        out = []
        for r in rows:
            out.append({"id": r[0], "title": r[1], "messageCount": r[5], "model": r[2], "createdAt": r[3], "updatedAt": r[4]})
        return out
    except Exception:
        return []

def get_chat(workspace_path: str, chat_id: str, include_traces: bool = True, include_usage: bool = True) -> dict | None:
    if not chat_id:
        return None
    _validate_chat_id(chat_id)
    db = _get_db()
    cur = db.execute("SELECT id, workspace_id, workspace_path, title, model, created_at, updated_at, pinned FROM chats WHERE id=?", (chat_id,))
    row = cur.fetchone()
    if not row:
        return None
    # Load messages
    if include_traces:
        cur = db.execute("SELECT id, role, content, timestamp, is_trace, metadata_json FROM messages WHERE chat_id=? ORDER BY timestamp ASC", (chat_id,))
    else:
        cur = db.execute("SELECT id, role, content, timestamp, is_trace, metadata_json FROM messages WHERE chat_id=? AND is_trace=0 ORDER BY timestamp ASC", (chat_id,))
    msgs = []
    for mid, role, content, ts, is_trace, meta_json in cur.fetchall():
        try:
            meta = json.loads(meta_json) if meta_json else {}
        except Exception:
            meta = {}
        # Ensure isTrace flag in metadata for frontend
        if is_trace and "isTrace" not in meta:
            meta["isTrace"] = True
        msgs.append({"id": mid, "role": role, "content": content, "timestamp": ts, "metadata": meta} if meta else {"id": mid, "role": role, "content": content, "timestamp": ts})
    chat = {"id": row[0], "title": row[3], "model": row[4], "createdAt": row[5], "updatedAt": row[6], "messages": msgs, "isPinned": bool(row[7])}
    chat["_traces_loaded"] = include_traces
    chat["_usage_loaded"] = include_usage
    return chat

def create_chat(workspace_path: str, title: str = "", model: str = "Automatic Fallback") -> dict:
    db = _get_db()
    chat_id = f"chat-{uuid.uuid4().hex[:12]}"
    now = datetime.now().isoformat()
    ws_id = workspace_path or GENERAL_WORKSPACE_ID
    if workspace_path not in (GENERAL_WORKSPACE_ID, HOUSE_WORKSPACE_ID, PERSONAL_WORKSPACE_ID, DFD_WORKSPACE_ID, '', None):
        try:
            from server.store.workspace_registry import _load_registry
            reg = _load_registry()
            norm = os.path.normpath(workspace_path).lower()
            for ws in reg.get("workspaces", []):
                if os.path.normpath(ws.get("path", "")).lower() == norm:
                    ws_id = ws.get("id")
                    break
        except Exception:
            pass
    # Resolve ws_id for virtual
    if workspace_path == HOUSE_WORKSPACE_ID:
        ws_id = HOUSE_WORKSPACE_ID
    elif workspace_path == PERSONAL_WORKSPACE_ID:
        ws_id = PERSONAL_WORKSPACE_ID
    elif workspace_path == DFD_WORKSPACE_ID:
        ws_id = DFD_WORKSPACE_ID
    elif workspace_path in ('', GENERAL_WORKSPACE_ID):
        ws_id = GENERAL_WORKSPACE_ID
    db.execute("INSERT INTO chats (id, workspace_id, workspace_path, title, model, created_at, updated_at, message_count, mode) VALUES (?,?,?,?,?,?,?,?,?)",
               (chat_id, ws_id, workspace_path or "", title or "New Chat", model, now, now, 0, "build"))
    db.commit()
    # Create empty JSONL
    jpath = _jsonl_path(workspace_path or GENERAL_WORKSPACE_ID, chat_id)
    _ensure_dir(os.path.dirname(jpath))
    open(jpath, "a").close()
    return {"id": chat_id, "title": title or "New Chat", "model": model, "createdAt": now, "updatedAt": now, "messages": []}

def save_chat(workspace_path: str, chat_id: str, data: dict) -> bool:
    _validate_chat_id(chat_id)
    lock = _chat_lock(chat_id)
    with lock:
        db = _get_db()
        try:
            data["updatedAt"] = datetime.now().isoformat()
            messages = data.get("messages", [])
            messages.sort(key=lambda m: m.get("timestamp", ""))
            for idx, m in enumerate(messages):
                m["index"] = idx
            chat_messages = []
            trace_messages = []
            for m in messages:
                if m.get("metadata", {}).get("isTrace"):
                    trace_messages.append(m)
                else:
                    chat_messages.append(m)
            # Update chats row
            title = data.get("title", "Untitled")
            model = data.get("model", "Automatic Fallback")
            is_pinned = 1 if data.get("isPinned") else 0
            db.execute("UPDATE chats SET title=?, model=?, updated_at=?, message_count=?, pinned=?, tasks_json=? WHERE id=?", (title, model, data["updatedAt"], len(messages), is_pinned,
                        json.dumps(data.get("tasks", []), ensure_ascii=False), chat_id))
            if db.execute("SELECT changes()").fetchone()[0] == 0:
                # Insert if not exists
                ws_id = workspace_path or GENERAL_WORKSPACE_ID
                db.execute("INSERT OR IGNORE INTO chats (id, workspace_id, workspace_path, title, model, created_at, updated_at, message_count, pinned, tasks_json) VALUES (?,?,?,?,?,?,?,?,?,?)",
                           (chat_id, ws_id, workspace_path or "", title, model, data.get("createdAt", data["updatedAt"]), data["updatedAt"], len(messages), is_pinned,
                            json.dumps(data.get("tasks", []), ensure_ascii=False)))
            # Replace messages
            db.execute("DELETE FROM messages WHERE chat_id=?", (chat_id,))
            jpath = _jsonl_path(workspace_path or GENERAL_WORKSPACE_ID, chat_id)
            _ensure_dir(os.path.dirname(jpath))
            # Write JSONL (all messages, trace flag preserved)
            with open(jpath, "w", encoding="utf-8") as jf:
                for m in messages:
                    entry = {
                        "id": m.get("id", f"msg-{uuid.uuid4().hex[:8]}"),
                        "role": m.get("role", "user"),
                        "content": m.get("content", ""),
                        "timestamp": m.get("timestamp", datetime.now().isoformat()),
                        "isTrace": bool(m.get("metadata", {}).get("isTrace")),
                        "metadata": m.get("metadata", {}),
                    }
                    jf.write(json.dumps(entry, ensure_ascii=False) + "\n")
                    db.execute("INSERT INTO messages (id, chat_id, role, content, timestamp, is_trace, metadata_json) VALUES (?,?,?,?,?,?,?)",
                               (entry["id"], chat_id, entry["role"], entry["content"], entry["timestamp"], 1 if entry["isTrace"] else 0, json.dumps(entry["metadata"])))
            db.commit()
            return True
        except Exception as e:
            print(f"[CHAT_STORE] save_chat {chat_id} failed: {e}")
            return False

def add_message(workspace_path: str, chat_id: str, role: str, content: str, metadata: dict = None) -> dict | None:
    chat = get_chat(workspace_path, chat_id, include_traces=True, include_usage=False)
    if chat is None:
        return None
    msg = {"id": f"msg-{uuid.uuid4().hex[:8]}", "role": role, "content": content, "timestamp": datetime.now().isoformat()}
    if metadata:
        msg["metadata"] = metadata
    chat.setdefault("messages", []).append(msg)
    if role == "user" and chat.get("title") in ("", "New Chat", "Untitled"):
        chat["title"] = content[:60] + ("..." if len(content) > 60 else "")
    chat["updatedAt"] = datetime.now().isoformat()
    save_chat(workspace_path, chat_id, chat)
    return msg

def delete_chat(workspace_path: str, chat_id: str) -> bool:
    _validate_chat_id(chat_id)
    lock = _chat_lock(chat_id)
    with lock:
        db = _get_db()
        db.execute("DELETE FROM chats WHERE id=?", (chat_id,))
        db.execute("DELETE FROM messages WHERE chat_id=?", (chat_id,))
        db.commit()
        # Clean per-chat lock map
        with _CHAT_LOCKS_LOCK:
            _CHAT_LOCKS.pop(chat_id, None)
    # Remove JSONL
    try:
        jpath = _jsonl_path(workspace_path or GENERAL_WORKSPACE_ID, chat_id)
        if os.path.isfile(jpath):
            os.remove(jpath)
    except Exception:
        pass
    return True

def update_chat_title(workspace_path: str, chat_id: str, title: str) -> bool:
    chat = get_chat(workspace_path, chat_id, include_traces=False, include_usage=False)
    if chat is None:
        return False
    chat["title"] = title
    return save_chat(workspace_path, chat_id, chat)

def get_chat_tasks(workspace_path: str, chat_id: str) -> list[dict]:
    """Read only the tasks metadata for a chat — no full message load.

    Tasks live in the chats row's tasks_json column (SQLite is canonical).
    Legacy chats saved before the migration keep their tasks inside the JSONL
    transcript; fall back to a JSONL scan only for those (one-shot per chat).
    """
    _validate_chat_id(chat_id)
    lock = _chat_lock(chat_id)
    with lock:
        db = _get_db()
        row = db.execute(
            "SELECT tasks_json FROM chats WHERE id=?",
            (chat_id,),
        ).fetchone()
    if row is not None and row[0]:
        try:
            return json.loads(row[0])
        except (TypeError, ValueError):
            pass
    # Legacy fallback: read tasks from the JSONL transcript (old format).
    try:
        with open(_jsonl_path(workspace_path or GENERAL_WORKSPACE_ID, chat_id), encoding="utf-8") as jf:
            for line in jf:
                line = line.strip()
                if not line:
                    continue
                try:
                    entry = json.loads(line)
                except ValueError:
                    continue
                meta = entry.get("metadata", {}) or {}
                if "tasks" in meta:
                    return meta["tasks"]
    except OSError:
        pass
    return []

def save_chat_tasks(workspace_path: str, chat_id: str, tasks: list[dict]) -> bool:
    """Persist only the tasks metadata — no full-chat rewrite."""
    _validate_chat_id(chat_id)
    lock = _chat_lock(chat_id)
    with lock:
        db = _get_db()
        db.execute(
            "UPDATE chats SET tasks_json=? WHERE id=?",
            (json.dumps(tasks, ensure_ascii=False), chat_id),
        )
        db.commit()
    try:
        from server.services.task_scheduler import notify_tasks_changed
        notify_tasks_changed(workspace_path or GENERAL_WORKSPACE_ID, chat_id)
    except Exception:
        pass
    return True
