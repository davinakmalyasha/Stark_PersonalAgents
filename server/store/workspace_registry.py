"""
Workspace registry — extracted from Desktop/workspace_manager.py for Lane A.
Keeps workspaces.json + virtual workspaces (_general/_house/_personal) without chat I/O.
Chat I/O lives in chat_store.py (hybrid JSONL+SQLite).
"""
import os
import json
import re
import uuid
import time
import threading
from datetime import datetime

REGISTRY_PATH = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "workspaces.json")
GENERAL_WORKSPACE_ID = "_general"
GENERAL_CHAT_DIR = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats", GENERAL_WORKSPACE_ID)
HOUSE_WORKSPACE_ID = "_house"
HOUSE_CHAT_DIR = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats", HOUSE_WORKSPACE_ID)
PERSONAL_WORKSPACE_ID = "_personal"
PERSONAL_CHAT_DIR = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats", PERSONAL_WORKSPACE_ID)
DFD_WORKSPACE_ID = "_dfd"
DFD_CHAT_DIR = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats", DFD_WORKSPACE_ID)
VIRTUAL_WORKSPACE_IDS = {GENERAL_WORKSPACE_ID, HOUSE_WORKSPACE_ID, PERSONAL_WORKSPACE_ID, DFD_WORKSPACE_ID}

# Phase 8: environment probe cache — probe_environment spawns 6 subprocesses
# (composer/npm/php/python/go/git, each shell=True, 2s timeout) + 4 socket
# probes. Those are expensive and rarely change; cache per workspace and only
# re-probe after a 5-minute TTL (or on explicit add_workspace).
_PROBE_TTL = 300  # seconds
_probe_cache: dict[str, tuple[float, dict]] = {}
_probe_cache_lock = threading.RLock()

def _ensure_dir(path: str) -> None:
    os.makedirs(path, exist_ok=True)

def _load_registry() -> dict:
    _ensure_dir(os.path.dirname(REGISTRY_PATH))
    if not os.path.isfile(REGISTRY_PATH):
        return {"workspaces": []}
    try:
        with open(REGISTRY_PATH, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return {"workspaces": []}

def _save_registry(reg: dict) -> None:
    _ensure_dir(os.path.dirname(REGISTRY_PATH))
    with open(REGISTRY_PATH, "w", encoding="utf-8") as f:
        json.dump(reg, f, indent=2)
    try:
        from routing.cache import invalidate as _cache_invalidate
        _cache_invalidate()
    except Exception:
        pass

def _resolve_workspace_id(workspace_path: str) -> str | None:
    reg = _load_registry()
    norm = os.path.normpath(workspace_path).lower() if workspace_path else ""
    for ws in reg.get("workspaces", []):
        if os.path.normpath(ws.get("path", "")).lower() == norm:
            return ws.get("id")
    return None

def list_workspaces() -> list[dict]:
    """Return workspaces ordered by most recent chat activity (uses chat_store.list_chats for counts)."""
    # Lazy import to avoid circular
    from server.store.chat_store import list_chats as _list_chats
    reg = _load_registry()
    general_chats = _list_chats(GENERAL_WORKSPACE_ID)
    house_chats = _list_chats(HOUSE_WORKSPACE_ID)
    personal_chats = _list_chats(PERSONAL_WORKSPACE_ID)
    dfd_chats = _list_chats(DFD_WORKSPACE_ID)
    workspaces = [
        {
            "id": GENERAL_WORKSPACE_ID,
            "name": "General",
            "path": "",
            "exists": True,
            "chatCount": len(general_chats),
            "addedAt": "",
            "updatedAt": max((chat.get("updatedAt", "") for chat in general_chats), default=""),
            "isGeneral": True,
            "workspaceKind": "general",
        },
        {
            "id": HOUSE_WORKSPACE_ID,
            "name": "House",
            "path": HOUSE_WORKSPACE_ID,
            "exists": True,
            "chatCount": len(house_chats),
            "addedAt": "",
            "updatedAt": max((chat.get("updatedAt", "") for chat in house_chats), default=""),
            "isGeneral": False,
            "isHouse": True,
            "workspaceKind": "house",
        },
        {
            "id": PERSONAL_WORKSPACE_ID,
            "name": "Personal",
            "path": PERSONAL_WORKSPACE_ID,
            "exists": True,
            "chatCount": len(personal_chats),
            "addedAt": "",
            "updatedAt": max((chat.get("updatedAt", "") for chat in personal_chats), default=""),
            "isGeneral": False,
            "isPersonal": True,
            "workspaceKind": "personal",
        },
        {
            "id": DFD_WORKSPACE_ID,
            "name": "DFD",
            "path": DFD_WORKSPACE_ID,
            "exists": True,
            "chatCount": len(dfd_chats),
            "addedAt": "",
            "updatedAt": max((chat.get("updatedAt", "") for chat in dfd_chats), default=""),
            "isGeneral": False,
            "isDfd": True,
            "workspaceKind": "dfd",
        },
    ]
    projects = []
    for ws in reg.get("workspaces", []):
        ws_path = ws.get("path", "")
        exists = os.path.isdir(ws_path) if ws_path else False
        chats = _list_chats(ws_path) if exists else []
        latest_activity = max(
            (chat.get("updatedAt", "") or chat.get("createdAt", "") for chat in chats),
            default=ws.get("addedAt", ""),
        )
        projects.append({
            "id": ws.get("id", ""),
            "name": ws.get("name", os.path.basename(ws_path)),
            "path": ws_path,
            "exists": exists,
            "chatCount": len(chats),
            "addedAt": ws.get("addedAt", ""),
            "updatedAt": latest_activity,
            "workspaceKind": "project",
            "isGeneral": False,
        })
    projects.sort(key=lambda p: p.get("updatedAt", ""), reverse=True)
    return workspaces + projects

def add_workspace(folder_path: str, name: str = "") -> dict:
    folder_path = os.path.normpath(os.path.abspath(folder_path))
    if not os.path.isdir(folder_path):
        raise ValueError(f"Folder not found: {folder_path}")
    reg = _load_registry()
    for ws in reg.get("workspaces", []):
        if os.path.normpath(ws.get("path", "")).lower() == folder_path.lower():
            return ws
    ws_id = f"ws-{uuid.uuid4().hex[:8]}"
    central_chats_root = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats")
    active_ids = {ws.get("id") for ws in reg.get("workspaces", [])}
    if os.path.isdir(central_chats_root):
        for dir_name in os.listdir(central_chats_root):
            if not dir_name.startswith("ws-") or dir_name in active_ids:
                continue
            dir_path = os.path.join(central_chats_root, dir_name)
            if not os.path.isdir(dir_path):
                continue
            path_file = os.path.join(dir_path, "workspace_path.txt")
            if os.path.isfile(path_file):
                try:
                    with open(path_file, "r", encoding="utf-8") as f:
                        saved_path = f.read().strip()
                    if os.path.normpath(saved_path).lower() == folder_path.lower():
                        ws_id = dir_name
                        break
                except Exception:
                    pass
    ws_name = name or os.path.basename(folder_path)
    entry = {"id": ws_id, "name": ws_name, "path": folder_path, "addedAt": datetime.now().isoformat()}
    reg.setdefault("workspaces", []).append(entry)
    _save_registry(reg)
    from server.store.chat_store import _chat_dir
    _ensure_dir(_chat_dir(folder_path))
    try:
        probe_environment(folder_path)
    except Exception:
        pass
    return entry

def remove_workspace(workspace_id: str) -> bool:
    import shutil
    reg = _load_registry()
    before = len(reg.get("workspaces", []))
    reg["workspaces"] = [w for w in reg.get("workspaces", []) if w.get("id") != workspace_id]
    if len(reg["workspaces"]) < before:
        _save_registry(reg)
        central_dir = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats", workspace_id)
        if os.path.isdir(central_dir):
            try:
                shutil.rmtree(central_dir)
            except Exception:
                pass
        return True
    return False

def get_workspace(workspace_id: str) -> dict | None:
    if workspace_id == GENERAL_WORKSPACE_ID:
        from server.store.chat_store import list_chats as _list_chats
        return {"id": GENERAL_WORKSPACE_ID, "name": "General", "path": "", "exists": True, "chatCount": len(_list_chats(GENERAL_WORKSPACE_ID)), "isGeneral": True, "workspaceKind": "general"}
    if workspace_id == HOUSE_WORKSPACE_ID:
        from server.store.chat_store import list_chats as _list_chats
        return {"id": HOUSE_WORKSPACE_ID, "name": "House", "path": HOUSE_WORKSPACE_ID, "exists": True, "chatCount": len(_list_chats(HOUSE_WORKSPACE_ID)), "isHouse": True, "workspaceKind": "house"}
    if workspace_id == PERSONAL_WORKSPACE_ID:
        from server.store.chat_store import list_chats as _list_chats
        return {"id": PERSONAL_WORKSPACE_ID, "name": "Personal", "path": PERSONAL_WORKSPACE_ID, "exists": True, "chatCount": len(_list_chats(PERSONAL_WORKSPACE_ID)), "isPersonal": True, "workspaceKind": "personal"}
    if workspace_id == DFD_WORKSPACE_ID:
        from server.store.chat_store import list_chats as _list_chats
        return {"id": DFD_WORKSPACE_ID, "name": "DFD", "path": DFD_WORKSPACE_ID, "exists": True, "chatCount": len(_list_chats(DFD_WORKSPACE_ID)), "isDfd": True, "workspaceKind": "dfd"}
    for ws in list_workspaces():
        if ws["id"] == workspace_id:
            return ws
    return None

def cleanup_orphaned_chat_dirs() -> int:
    import shutil
    reg = _load_registry()
    active_ids = {ws.get("id", "") for ws in reg.get("workspaces", [])}
    # Also keep virtual ids
    active_ids.update(VIRTUAL_WORKSPACE_IDS)
    central_chats_root = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats")
    removed = 0
    if os.path.isdir(central_chats_root):
        for dir_name in os.listdir(central_chats_root):
            if not dir_name.startswith("ws-") and dir_name not in ("_unregistered",):
                # Keep virtual dirs, already in active_ids
                if dir_name in VIRTUAL_WORKSPACE_IDS:
                    continue
                if dir_name in active_ids:
                    continue
            if dir_name in active_ids:
                continue
            dir_path = os.path.join(central_chats_root, dir_name)
            if os.path.isdir(dir_path) and dir_name.startswith("ws-"):
                try:
                    shutil.rmtree(dir_path)
                    removed += 1
                    print(f"[CLEANUP] Removed orphaned chat directory: {dir_name}")
                except Exception as e:
                    print(f"[CLEANUP] Failed to remove {dir_name}: {e}")
    return removed

def init_default_workspace() -> dict | None:
    default_path = r"d:\MyProject\LangChain"
    workspaces = list_workspaces()
    for ws in workspaces:
        if os.path.normpath(ws.get("path", "")) == os.path.normpath(default_path):
            try:
                probe_environment(default_path)
            except Exception:
                pass
            return ws
    if workspaces:
        # If other workspaces exist (including virtual), do not recreate default? Keep legacy behavior: only if no project workspaces
        # Count real projects only
        real_projects = [w for w in workspaces if w.get("id", "").startswith("ws-")]
        if real_projects:
            return None
    ws = add_workspace(default_path, "LangChain")
    try:
        probe_environment(default_path)
    except Exception:
        pass
    return ws

def _probe_environment_uncached(workspace_path: str) -> dict:
    import subprocess
    import socket
    tools = ["composer", "npm", "php", "python", "go", "git"]
    tool_status = {}
    for tool in tools:
        try:
            res = subprocess.run(f"{tool} --version", shell=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=2)
            if res.returncode == 0 or res.stdout or res.stderr:
                out = (res.stdout or res.stderr).strip().split('\n')[0]
                tool_status[tool] = {"available": True, "version": out[:100]}
            else:
                tool_status[tool] = {"available": False}
        except Exception:
            if tool == "python":
                try:
                    res = subprocess.run("python -V", shell=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=2)
                    if res.returncode == 0 or res.stdout or res.stderr:
                        out = (res.stdout or res.stderr).strip()
                        tool_status[tool] = {"available": True, "version": out}
                        continue
                except Exception:
                    pass
            tool_status[tool] = {"available": False}
    ports = [8000, 3000, 5173, 8080]
    port_status = {}
    for port in ports:
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(0.5)
        res = s.connect_ex(('127.0.0.1', port))
        s.close()
        port_status[str(port)] = "IN_USE" if res == 0 else "FREE"
    return {"cli_tools": tool_status, "ports": port_status, "timestamp": datetime.now().isoformat()}

def probe_environment(workspace_path: str) -> dict:
    """Return a cached environment probe (6 CLI subprocesses + 4 socket probes).

    Cached per workspace for _PROBE_TTL seconds — repeated calls (startup,
    add_workspace, the per-run sync path) hit memory instead of spawning ~6
    Windows subprocesses every time. The result is pure environment
    introspection; staleness is bounded by the TTL.
    """
    now = time.time()
    with _probe_cache_lock:
        hit = _probe_cache.get(workspace_path or "")
        if hit and (now - hit[0]) < _PROBE_TTL:
            return hit[1]
    profile = _probe_environment_uncached(workspace_path)
    with _probe_cache_lock:
        _probe_cache[workspace_path or ""] = (now, profile)
    return profile

def get_workspace_rules_and_profile(workspace_path: str) -> str:
    import json
    server_dir = r"d:\MyProject\LangChain"
    global_path = os.path.join(server_dir, ".deep_agents", "user_profile.json")
    ws_id = _resolve_workspace_id(workspace_path) if workspace_path else None
    rules_path = None
    if ws_id:
        rules_path = os.path.join(server_dir, ".deep_agents", "rules", f"{ws_id}.json")
    global_data = {}
    if os.path.isfile(global_path):
        try:
            with open(global_path, "r", encoding="utf-8") as f:
                global_data = json.load(f)
        except Exception:
            pass
    local_data = {}
    if rules_path and os.path.isfile(rules_path):
        try:
            with open(rules_path, "r", encoding="utf-8") as f:
                local_data = json.load(f)
        except Exception:
            pass
    sections = []
    user_info = global_data.get("user_info", {})
    if user_info:
        sections.append("## User Information")
        for k, v in user_info.items():
            sections.append(f"- {k.replace('_', ' ').title()}: {v}")
    global_rules = global_data.get("global_rules", [])
    if global_rules:
        sections.append("## Global Development Rules")
        for rule in global_rules:
            sections.append(f"- {rule}")
    workspace_rules = local_data.get("workspace_rules", [])
    tech_stack = local_data.get("stack", {})
    if tech_stack:
        sections.append("## Workspace Stack")
        for layer, techs in tech_stack.items():
            techs_str = ", ".join(techs) if isinstance(techs, list) else str(techs)
            sections.append(f"- {layer.title()}: {techs_str}")
    if workspace_rules:
        sections.append("## Workspace Rules")
        for rule in workspace_rules:
            sections.append(f"- {rule}")
    if sections:
        return "\n".join(["=========================================", "CRITICAL: USER PREFERENCES & RULES", "=========================================", ""] + sections + ["", "========================================="])
    return ""

def get_client_name(workspace_path: str = "") -> str:
    server_dir = r"d:\MyProject\LangChain"
    global_path = os.path.join(server_dir, ".deep_agents", "user_profile.json")
    if os.path.isfile(global_path):
        try:
            with open(global_path, "r", encoding="utf-8") as f:
                data = json.load(f)
                name = data.get("user_info", {}).get("name")
                if name:
                    return name
        except Exception:
            pass
    return "Davin Akmal Yasha"
