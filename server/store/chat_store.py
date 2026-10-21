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
        cols.append("tasks_json")
    if "parent_chat_id" not in cols:
        db.execute("ALTER TABLE chats ADD COLUMN parent_chat_id TEXT")
        db.commit()
        cols.append("parent_chat_id")
    if "parent_workspace_id" not in cols:
        db.execute("ALTER TABLE chats ADD COLUMN parent_workspace_id TEXT")
        db.commit()
        cols.append("parent_workspace_id")
    if "token_usage_json" not in cols:
        db.execute("ALTER TABLE chats ADD COLUMN token_usage_json TEXT")
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

def find_chat_workspace_id(chat_id: str) -> str:
    """Best-effort lookup of a chat's workspace_id (for parent linkage).

    Returns "" when unknown. Never raises — callers use it to fill
    parent_workspace_id when only parent_chat_id is known (e.g. a child
    spawned in a different workspace than its parent).
    """
    if not chat_id:
        return ""
    try:
        db = _get_db()
        cur = db.execute("SELECT workspace_id FROM chats WHERE id=?", (chat_id,))
        row = cur.fetchone()
        return str(row[0] or "") if row else ""
    except Exception:
        return ""


def _resolve_parent_workspace_id(parent_chat_id: str, hint: str = "") -> str:
    if hint:
        return hint
    return find_chat_workspace_id(parent_chat_id)


_PARENT_BACKFILL_DONE = False

def _backfill_parents() -> None:
    global _PARENT_BACKFILL_DONE
    if _PARENT_BACKFILL_DONE:
        return
    _PARENT_BACKFILL_DONE = True
    try:
        ledger = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))), ".agent-memory", "orchestrator_jobs.jsonl")
        # Fallback to harness root
        if not os.path.isfile(ledger):
            ledger = os.path.join(r"d:\MyProject\LangChain", ".agent-memory", "orchestrator_jobs.jsonl")
        if not os.path.isfile(ledger):
            return
        db = _get_db()
        # Only backfill chats where parent is still null
        updated = 0
        with open(ledger, "r", encoding="utf-8") as f:
            for line in f:
                line=line.strip()
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                except Exception:
                    continue
                child = rec.get("chat_id") or rec.get("chatId")
                parent = rec.get("parent") or rec.get("parent_chat_id")
                ws = rec.get("workspace") or ""
                if not child or not parent:
                    continue
                try:
                    cur = db.execute("SELECT parent_chat_id, parent_workspace_id FROM chats WHERE id=?", (child,))
                    row = cur.fetchone()
                    if row and not row[0]:
                        parent_ws = find_chat_workspace_id(parent)
                        db.execute("UPDATE chats SET parent_chat_id=?, parent_workspace_id=? WHERE id=?", (parent, parent_ws or None, child))
                        updated += 1
                    elif row and row[0] and not row[1]:
                        # Parent known but workspace missing (older rows saved
                        # with parent_workspace_id="") — heal it in place.
                        parent_ws = find_chat_workspace_id(row[0])
                        if parent_ws:
                            db.execute("UPDATE chats SET parent_workspace_id=? WHERE id=?", (parent_ws, child))
                            updated += 1
                except Exception:
                    continue
        if updated:
            db.commit()
    except Exception:
        pass

def list_chats(workspace_path: str) -> list[dict]:
    _backfill_parents()
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
        cur = db.execute(
            "SELECT c.id, c.title, c.model, c.created_at, c.updated_at, c.message_count,"
            " c.parent_chat_id, c.parent_workspace_id,"
            " (SELECT m.content FROM messages m WHERE m.chat_id=c.id"
            "  ORDER BY m.timestamp DESC, m.rowid DESC LIMIT 1) AS last_message,"
            " (SELECT m.role FROM messages m WHERE m.chat_id=c.id"
            "  ORDER BY m.timestamp DESC, m.rowid DESC LIMIT 1) AS last_role"
            " FROM chats c WHERE c.workspace_id=? ORDER BY c.updated_at DESC",
            (ws_id,),
        )
        rows = cur.fetchall()
        out = []
        for r in rows:
            item = {"id": r[0], "title": r[1], "messageCount": r[5], "model": r[2], "createdAt": r[3], "updatedAt": r[4]}
            if r[6]:
                item["parentChatId"] = r[6]
            if r[7]:
                item["parentWorkspaceId"] = r[7]
            if r[8]:
                snippet = " ".join(str(r[8]).split())
                item["lastMessage"] = snippet[:180]
            if r[9]:
                item["lastRole"] = r[9]
            out.append(item)
        return out
    except Exception:
        return []

def get_chat_workspace_path(chat_id: str) -> str:
    """Return the stored workspace path for one chat ("" for General)."""
    if not chat_id:
        return ""
    _validate_chat_id(chat_id)
    db = _get_db()
    row = db.execute("SELECT workspace_path FROM chats WHERE id=?", (chat_id,)).fetchone()
    return (row[0] or "") if row else ""


def get_chat(workspace_path: str, chat_id: str, include_traces: bool = True, include_usage: bool = True) -> dict | None:
    if not chat_id:
        return None
    _validate_chat_id(chat_id)
    db = _get_db()
    cur = db.execute("SELECT id, workspace_id, workspace_path, title, model, created_at, updated_at, pinned, token_usage_json, parent_chat_id, parent_workspace_id FROM chats WHERE id=?", (chat_id,))
    row = cur.fetchone()
    if not row:
        return None
    # Load messages
    if include_traces:
        cur = db.execute("SELECT id, role, content, timestamp, is_trace, metadata_json FROM messages WHERE chat_id=? ORDER BY timestamp ASC, rowid ASC", (chat_id,))
    else:
        cur = db.execute("SELECT id, role, content, timestamp, is_trace, metadata_json FROM messages WHERE chat_id=? AND is_trace=0 ORDER BY timestamp ASC, rowid ASC", (chat_id,))
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
    if len(row) > 9 and row[9]:
        chat["parentChatId"] = row[9]
    if len(row) > 10 and row[10]:
        chat["parentWorkspaceId"] = row[10]
    if row[8]:
        try:
            chat["token_usage"] = json.loads(row[8])
        except Exception:
            pass
    chat["_traces_loaded"] = include_traces
    chat["_usage_loaded"] = include_usage
    return chat

def create_chat(workspace_path: str, title: str = "", model: str = "Automatic Fallback", parent_chat_id: str = "", parent_workspace_id: str = "") -> dict:
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
    # Cross-workspace hierarchy: a spawned child usually knows its parent chat
    # id but not the parent's workspace. Resolve it so the sidebar can group
    # orchestrator → workspace-agent → sub-delegation chains anywhere.
    resolved_parent_ws = _resolve_parent_workspace_id(parent_chat_id or "", parent_workspace_id or "")
    db.execute("INSERT INTO chats (id, workspace_id, workspace_path, title, model, created_at, updated_at, message_count, mode, parent_chat_id, parent_workspace_id) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
               (chat_id, ws_id, workspace_path or "", title or "New Chat", model, now, now, 0, "build", parent_chat_id or None, resolved_parent_ws or None))
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
            title = data.get("title", "Untitled")
            model = data.get("model", "Automatic Fallback")
            is_pinned = 1 if data.get("isPinned") else 0
            tu_json = json.dumps(data.get("token_usage"), ensure_ascii=False) if data.get("token_usage") else None

            # Safety guard: a chat loaded with include_traces=False does NOT
            # contain the agent/trace messages. Rewriting messages from that
            # view is how earlier replies used to disappear on every run. If
            # the caller only has a partial view, update metadata only.
            if data.get("_traces_loaded") is False:
                if data.get("_usage_loaded") is False:
                    db.execute(
                        "UPDATE chats SET title=?, model=?, updated_at=?, pinned=?, tasks_json=? WHERE id=?",
                        (title, model, data["updatedAt"], is_pinned,
                         json.dumps(data.get("tasks", []), ensure_ascii=False), chat_id),
                    )
                else:
                    db.execute(
                        "UPDATE chats SET title=?, model=?, updated_at=?, pinned=?, tasks_json=?, token_usage_json=? WHERE id=?",
                        (title, model, data["updatedAt"], is_pinned,
                         json.dumps(data.get("tasks", []), ensure_ascii=False), tu_json, chat_id),
                    )
                db.commit()
                return True

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
            # Update chats row. When the caller loaded the chat without usage,
            # preserve the existing token_usage_json instead of nulling it.
            if data.get("_usage_loaded") is False:
                db.execute("UPDATE chats SET title=?, model=?, updated_at=?, message_count=?, pinned=?, tasks_json=? WHERE id=?", (title, model, data["updatedAt"], len(messages), is_pinned,
                            json.dumps(data.get("tasks", []), ensure_ascii=False), chat_id))
            else:
                db.execute("UPDATE chats SET title=?, model=?, updated_at=?, message_count=?, pinned=?, tasks_json=?, token_usage_json=? WHERE id=?", (title, model, data["updatedAt"], len(messages), is_pinned,
                            json.dumps(data.get("tasks", []), ensure_ascii=False), tu_json, chat_id))
            if db.execute("SELECT changes()").fetchone()[0] == 0:
                # Insert if not exists — preserve parent linkage (spawned-child
                # grouping) instead of dropping it on this fallback path.
                ws_id = workspace_path or GENERAL_WORKSPACE_ID
                parent_id = data.get("parentChatId") or data.get("parent_chat_id")
                parent_ws = data.get("parentWorkspaceId") or data.get("parent_workspace_id")
                db.execute("INSERT OR IGNORE INTO chats (id, workspace_id, workspace_path, title, model, created_at, updated_at, message_count, pinned, tasks_json, token_usage_json, parent_chat_id, parent_workspace_id) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                           (chat_id, ws_id, workspace_path or "", title, model, data.get("createdAt", data["updatedAt"]), data["updatedAt"], len(messages), is_pinned,
                            json.dumps(data.get("tasks", []), ensure_ascii=False), tu_json, parent_id, parent_ws))
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

def update_chat_token_usage(workspace_path: str, chat_id: str, token_usage: dict) -> bool:
    """Persist token usage without touching the message table.

    The previous implementation loaded the chat with ``include_traces=False``
    and called ``save_chat`` — a full message rewrite that silently deleted
    every assistant/trace message on each run. Token accounting is a metadata
    concern; it must never rewrite the conversation.
    """
    _validate_chat_id(chat_id)
    with _chat_lock(chat_id):
        db = _get_db()
        try:
            db.execute(
                "UPDATE chats SET token_usage_json=?, updated_at=? WHERE id=?",
                (json.dumps(token_usage or {}, ensure_ascii=False),
                 datetime.now().isoformat(), chat_id),
            )
            db.commit()
            return True
        except Exception as e:
            print(f"[CHAT_STORE] update_chat_token_usage {chat_id} failed: {e}")
            return False


def add_message(workspace_path: str, chat_id: str, role: str, content: str, metadata: dict = None) -> dict | None:
    """Atomically append one message (SQLite + JSONL).

    The whole read-decide-write path holds the per-chat lock. Appending only
    the new row (instead of rewriting every message) keeps concurrent writers
    from losing messages and keeps long chats cheap.
    """
    _validate_chat_id(chat_id)
    with _chat_lock(chat_id):
        db = _get_db()
        row = db.execute("SELECT title FROM chats WHERE id=?", (chat_id,)).fetchone()
        if not row:
            return None
        now = datetime.now().isoformat()
        msg = {
            "id": f"msg-{uuid.uuid4().hex[:8]}",
            "role": role,
            "content": content,
            "timestamp": now,
        }
        if metadata:
            msg["metadata"] = metadata
        is_trace = 1 if (metadata or {}).get("isTrace") else 0
        title = row[0] or ""
        new_title = title
        if role == "user" and title in ("", "New Chat", "Untitled", "New Conversation"):
            new_title = content[:60] + ("..." if len(content) > 60 else "")
        try:
            db.execute(
                "INSERT INTO messages (id, chat_id, role, content, timestamp, is_trace, metadata_json) VALUES (?,?,?,?,?,?,?)",
                (msg["id"], chat_id, role, content, now, is_trace, json.dumps(metadata or {})),
            )
            db.execute(
                "UPDATE chats SET title=?, updated_at=?, message_count=COALESCE(message_count, 0) + 1 WHERE id=?",
                (new_title, now, chat_id),
            )
            db.commit()
        except Exception as e:
            try:
                db.rollback()
            except Exception:
                pass
            print(f"[CHAT_STORE] add_message {chat_id} failed: {e}")
            return None

        # JSONL is the durable transcript; a failed append must not lose the
        # SQLite write, so it stays best-effort.
        try:
            jpath = _jsonl_path(workspace_path or GENERAL_WORKSPACE_ID, chat_id)
            _ensure_dir(os.path.dirname(jpath))
            entry = {
                "id": msg["id"], "role": role, "content": content,
                "timestamp": now, "isTrace": bool(is_trace),
                "metadata": metadata or {},
            }
            with open(jpath, "a", encoding="utf-8") as jf:
                jf.write(json.dumps(entry, ensure_ascii=False) + "\n")
        except Exception as e:
            print(f"[CHAT_STORE] add_message JSONL append {chat_id} failed: {e}")
        return msg

def truncate_from_message(workspace_path: str, chat_id: str, message_id: str) -> dict | None:
    """Remove one message and everything after it.

    Used by message-level rewind. The whole read/delete/JSONL rewrite holds the
    per-chat lock, so a concurrent append cannot slip a message into the gap.
    Returns the rewound message, the prompt text, and the removed rows.
    """
    _validate_chat_id(chat_id)
    if not message_id:
        return None
    with _chat_lock(chat_id):
        db = _get_db()
        rows = db.execute(
            "SELECT rowid, id, role, content, timestamp, is_trace, metadata_json "
            "FROM messages WHERE chat_id=? ORDER BY timestamp ASC, rowid ASC",
            (chat_id,),
        ).fetchall()
        target_idx = next((i for i, r in enumerate(rows) if r[1] == message_id), None)
        if target_idx is None:
            return None
        target = rows[target_idx]
        keep = rows[:target_idx]
        removed = rows[target_idx:]
        now = datetime.now().isoformat()
        try:
            placeholders = ",".join("?" for _ in removed)
            db.execute(
                f"DELETE FROM messages WHERE chat_id=? AND id IN ({placeholders})",
                (chat_id, *[r[1] for r in removed]),
            )
            db.execute(
                "UPDATE chats SET updated_at=?, message_count=? WHERE id=?",
                (now, len(keep), chat_id),
            )
            jpath = _jsonl_path(workspace_path or GENERAL_WORKSPACE_ID, chat_id)
            _ensure_dir(os.path.dirname(jpath))
            with open(jpath, "w", encoding="utf-8") as jf:
                for r in keep:
                    try:
                        meta = json.loads(r[6]) if r[6] else {}
                    except Exception:
                        meta = {}
                    jf.write(json.dumps({
                        "id": r[1], "role": r[2], "content": r[3],
                        "timestamp": r[4], "isTrace": bool(r[5]),
                        "metadata": meta,
                    }, ensure_ascii=False) + "\n")
            db.commit()
        except Exception as e:
            try:
                db.rollback()
            except Exception:
                pass
            print(f"[CHAT_STORE] truncate_from_message {chat_id} failed: {e}")
            return None
        return {
            "message": {"id": target[1], "role": target[2], "content": target[3]},
            "prompt": target[3],
            "removed": [{"id": r[1], "role": r[2]} for r in removed],
        }


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
    chat = get_chat(workspace_path, chat_id, include_traces=True, include_usage=True)
    if chat is None:
        return False
    chat["title"] = title
    return save_chat(workspace_path, chat_id, chat)
