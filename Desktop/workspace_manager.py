"""
Workspace Manager — Maps workspaces to real disk folders.
Each workspace stores chats as JSON in .deep_agents/chats/.
"""
import os
import json
import re
import uuid
import time
from datetime import datetime

# Global registry: where workspace metadata is stored
REGISTRY_PATH = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "workspaces.json")

# Virtual workspace for general (non-folder) chats
GENERAL_WORKSPACE_ID = "_general"
GENERAL_CHAT_DIR = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats", GENERAL_WORKSPACE_ID)


def _ensure_dir(path: str) -> None:
    os.makedirs(path, exist_ok=True)


def _load_registry() -> dict:
    """Load the workspace registry from disk."""
    _ensure_dir(os.path.dirname(REGISTRY_PATH))
    if not os.path.isfile(REGISTRY_PATH):
        return {"workspaces": []}
    try:
        with open(REGISTRY_PATH, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return {"workspaces": []}


def _save_registry(reg: dict) -> None:
    """Save the workspace registry to disk."""
    _ensure_dir(os.path.dirname(REGISTRY_PATH))
    with open(REGISTRY_PATH, "w", encoding="utf-8") as f:
        json.dump(reg, f, indent=2)


def _chat_dir(workspace_path: str) -> str:
    """Get the chat storage directory for a workspace."""
    # General workspace uses central storage, not a folder path
    if workspace_path == GENERAL_WORKSPACE_ID or workspace_path == '':
        _ensure_dir(GENERAL_CHAT_DIR)
        return GENERAL_CHAT_DIR
    workspace_path = os.path.normpath(os.path.abspath(workspace_path))
    
    # Try to find the workspace ID in the registry (case-insensitive for Windows)
    reg = _load_registry()
    for ws in reg.get("workspaces", []):
        if os.path.normpath(ws.get("path", "")).lower() == workspace_path.lower():
            ws_id = ws.get("id")
            central_dir = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats", ws_id)
            _ensure_dir(central_dir)
            # Save the workspace path to a metadata file for orphan matching
            path_file = os.path.join(central_dir, "workspace_path.txt")
            if not os.path.isfile(path_file):
                try:
                    with open(path_file, "w", encoding="utf-8") as f:
                        f.write(workspace_path)
                except Exception:
                    pass
            return central_dir
            
    # Fallback: use a central orphan directory instead of littering the project folder
    fallback_dir = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats", "_unregistered")
    _ensure_dir(fallback_dir)
    return fallback_dir


def _validate_chat_id(chat_id: str) -> None:
    """Validate chat_id to prevent path traversal."""
    if chat_id and not re.match(r'^[\w\-.]+$', chat_id):
        raise ValueError(f"Invalid chat_id: {chat_id}")


def _chat_path(workspace_path: str, chat_id: str) -> str:
    """Get the full path to a chat JSON file."""
    _validate_chat_id(chat_id)
    return os.path.join(_chat_dir(workspace_path), f"{chat_id}.json")


def _trace_path(workspace_path: str, chat_id: str) -> str:
    """Get the full path to a chat traces JSON file."""
    _validate_chat_id(chat_id)
    return os.path.join(_chat_dir(workspace_path), f"{chat_id}_traces.json")


def _usage_path(workspace_path: str, chat_id: str) -> str:
    """Get the full path to a chat usage JSON file."""
    _validate_chat_id(chat_id)
    return os.path.join(_chat_dir(workspace_path), f"{chat_id}_usage.json")


# ═══════════════════════════════════════════════════════════════════════════════
# Workspace CRUD
# ═══════════════════════════════════════════════════════════════════════════════

def list_workspaces() -> list[dict]:
    """Return workspaces ordered by most recent chat activity."""
    reg = _load_registry()
    general_chats = list_chats(GENERAL_WORKSPACE_ID)
    general_chat_count = len(general_chats)
    workspaces = [{
        "id": GENERAL_WORKSPACE_ID,
        "name": "General",
        "path": "",
        "exists": True,
        "chatCount": general_chat_count,
        "addedAt": "",
        "updatedAt": max((chat.get("updatedAt", "") for chat in general_chats), default=""),
        "isGeneral": True,
    }]
    projects = []
    for ws in reg.get("workspaces", []):
        ws_path = ws.get("path", "")
        exists = os.path.isdir(ws_path) if ws_path else False
        chats = list_chats(ws_path) if exists else []
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
        })
    projects.sort(key=lambda project: project.get("updatedAt", ""), reverse=True)
    return workspaces + projects


def add_workspace(folder_path: str, name: str = "") -> dict:
    """Register a new workspace folder. Reuses old ID if the folder was previously registered."""
    folder_path = os.path.normpath(os.path.abspath(folder_path))
    if not os.path.isdir(folder_path):
        raise ValueError(f"Folder not found: {folder_path}")

    reg = _load_registry()

    # Check if already registered (exact path match, case-insensitive)
    for ws in reg.get("workspaces", []):
        if os.path.normpath(ws.get("path", "")).lower() == folder_path.lower():
            return ws

    # Check for orphaned chat directories from a previous registration of this
    # folder. If we find one, reuse its workspace ID so old chats aren't lost.
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
            
            # Check workspace_path.txt if it exists
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

    entry = {
        "id": ws_id,
        "name": ws_name,
        "path": folder_path,
        "addedAt": datetime.now().isoformat(),
    }

    reg.setdefault("workspaces", []).append(entry)
    _save_registry(reg)

    # Initialize .deep_agents/chats directory
    _ensure_dir(_chat_dir(folder_path))

    # Probe environment
    try:
        probe_environment(folder_path)
    except Exception:
        pass

    return entry


def remove_workspace(workspace_id: str) -> bool:
    """Remove a workspace from the registry and clean up its chat directory."""
    import shutil
    reg = _load_registry()
    before = len(reg.get("workspaces", []))
    reg["workspaces"] = [w for w in reg.get("workspaces", []) if w.get("id") != workspace_id]
    if len(reg["workspaces"]) < before:
        _save_registry(reg)
        # Clean up the central chat directory so orphaned folders don't accumulate
        central_dir = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats", workspace_id)
        if os.path.isdir(central_dir):
            try:
                shutil.rmtree(central_dir)
            except Exception:
                pass
        return True
    return False


def get_workspace(workspace_id: str) -> dict | None:
    """Get a single workspace by ID."""
    if workspace_id == GENERAL_WORKSPACE_ID:
        return {
            "id": GENERAL_WORKSPACE_ID,
            "name": "General",
            "path": "",
            "exists": True,
            "chatCount": len(list_chats(GENERAL_WORKSPACE_ID)),
            "isGeneral": True,
        }
    for ws in list_workspaces():
        if ws["id"] == workspace_id:
            return ws
    return None


# ═══════════════════════════════════════════════════════════════════════════════
# Chat CRUD (per workspace)
# ═══════════════════════════════════════════════════════════════════════════════

def list_chats(workspace_path: str) -> list[dict]:
    """List all chats in a workspace folder."""
    cd = _chat_dir(workspace_path)
    if not os.path.isdir(cd):
        return []
    chats = []
    for fname in sorted(os.listdir(cd), reverse=True):
        if not fname.endswith(".json"):
            continue
        if fname.endswith("_traces.json") or fname.endswith("_usage.json"):
            continue
        fpath = os.path.join(cd, fname)
        try:
            with open(fpath, "r", encoding="utf-8") as f:
                data = json.load(f)
            chats.append({
                "id": data.get("id", fname.replace(".json", "")),
                "title": data.get("title", "Untitled"),
                "messageCount": len(data.get("messages", [])),
                "model": data.get("model", ""),
                "createdAt": data.get("createdAt", ""),
                "updatedAt": data.get("updatedAt", ""),
            })
        except Exception:
            pass
    chats.sort(key=lambda chat: chat.get("updatedAt", "") or chat.get("createdAt", ""), reverse=True)
    return chats


def create_chat(workspace_path: str, title: str = "", model: str = "Automatic Fallback") -> dict:
    """Create a new empty chat in a workspace."""
    _ensure_dir(_chat_dir(workspace_path))
    chat_id = f"chat-{uuid.uuid4().hex[:12]}"
    now = datetime.now().isoformat()
    data = {
        "id": chat_id,
        "title": title or "New Chat",
        "model": model,
        "createdAt": now,
        "updatedAt": now,
        "messages": [],
    }
    with open(_chat_path(workspace_path, chat_id), "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)
    return data


def get_chat(workspace_path: str, chat_id: str, include_traces: bool = True, include_usage: bool = True) -> dict | None:
    """Get a chat with all its messages combined from chat, trace, and usage files."""
    fpath = _chat_path(workspace_path, chat_id)
    if not os.path.isfile(fpath):
        return None
    try:
        with open(fpath, "r", encoding="utf-8") as f:
            chat = json.load(f)
    except Exception:
        return None

    chat["_traces_loaded"] = include_traces
    chat["_usage_loaded"] = include_usage

    # Load traces if requested and they exist
    if include_traces:
        tpath = _trace_path(workspace_path, chat_id)
        trace_messages = []
        if os.path.isfile(tpath):
            try:
                with open(tpath, "r", encoding="utf-8") as f:
                    trace_data = json.load(f)
                    trace_messages = trace_data.get("messages", [])
            except Exception:
                pass

        if trace_messages:
            combined = chat.get("messages", []) + trace_messages
            # Sort chronologically by timestamp
            combined.sort(key=lambda m: m.get("timestamp", ""))
            chat["messages"] = combined

    # Load token usage if requested and exists
    if include_usage:
        upath = _usage_path(workspace_path, chat_id)
        if os.path.isfile(upath):
            try:
                with open(upath, "r", encoding="utf-8") as f:
                    chat["token_usage"] = json.load(f)
            except Exception:
                pass

    return chat


def save_chat(workspace_path: str, chat_id: str, data: dict) -> bool:
    """Save (overwrite) a chat JSON file splitting out trace logs and usage to separate files."""
    _ensure_dir(_chat_dir(workspace_path))
    data["updatedAt"] = datetime.now().isoformat()
    
    # Store order index on all messages chronologically
    messages = data.get("messages", [])
    messages.sort(key=lambda m: m.get("timestamp", ""))
    for idx, m in enumerate(messages):
        m["index"] = idx

    # Split chat messages vs trace messages
    chat_messages = []
    trace_messages = []
    for m in messages:
        if m.get("metadata", {}).get("isTrace"):
            trace_messages.append(m)
        else:
            chat_messages.append(m)

    # Save main chat file without trace messages and token usage
    chat_data = data.copy()
    chat_data["messages"] = chat_messages
    if "token_usage" in chat_data:
        del chat_data["token_usage"]
    if "_traces_loaded" in chat_data:
        del chat_data["_traces_loaded"]
    if "_usage_loaded" in chat_data:
        del chat_data["_usage_loaded"]
    
    try:
        with open(_chat_path(workspace_path, chat_id), "w", encoding="utf-8") as f:
            json.dump(chat_data, f, indent=2)
            
        # Save trace file only if traces were loaded
        if data.get("_traces_loaded", True):
            tpath = _trace_path(workspace_path, chat_id)
            if trace_messages:
                trace_data = {
                    "chat_id": chat_id,
                    "messages": trace_messages
                }
                with open(tpath, "w", encoding="utf-8") as f:
                    json.dump(trace_data, f, indent=2)
            elif os.path.isfile(tpath):
                try:
                    os.remove(tpath)
                except Exception:
                    pass
                
        # Save token usage file only if usage was loaded
        if data.get("_usage_loaded", True):
            upath = _usage_path(workspace_path, chat_id)
            if "token_usage" in data:
                token_usage = data.get("token_usage")
                if token_usage:
                    with open(upath, "w", encoding="utf-8") as f:
                        json.dump(token_usage, f, indent=2)
                elif os.path.isfile(upath):
                    try:
                        os.remove(upath)
                    except Exception:
                        pass

        return True
    except Exception:
        return False


def add_message(workspace_path: str, chat_id: str, role: str, content: str, metadata: dict = None) -> dict | None:
    """Append a message to a chat and save."""
    chat = get_chat(workspace_path, chat_id, include_traces=True, include_usage=False)
    if chat is None:
        return None

    msg = {
        "id": f"msg-{uuid.uuid4().hex[:8]}",
        "role": role,
        "content": content,
        "timestamp": datetime.now().isoformat(),
    }
    if metadata:
        msg["metadata"] = metadata

    chat.setdefault("messages", []).append(msg)

    # Auto-title from first user message
    if role == "user" and chat.get("title") in ("", "New Chat", "Untitled"):
        chat["title"] = content[:60] + ("..." if len(content) > 60 else "")

    chat["updatedAt"] = datetime.now().isoformat()
    save_chat(workspace_path, chat_id, chat)
    return msg


def get_chat_tasks(workspace_path: str, chat_id: str) -> list[dict]:
    """Return persisted task definitions for a chat."""
    chat = get_chat(workspace_path, chat_id, include_traces=False, include_usage=False)
    return (chat or {}).get("tasks", [])


def save_chat_tasks(workspace_path: str, chat_id: str, tasks: list[dict]) -> bool:
    """Persist task definitions alongside the chat JSON."""
    chat = get_chat(workspace_path, chat_id, include_traces=False, include_usage=False)
    if chat is None:
        return False
    chat["tasks"] = tasks
    return save_chat(workspace_path, chat_id, chat)


def delete_chat(workspace_path: str, chat_id: str) -> bool:
    """Delete a chat JSON file and its trace/usage files."""
    fpath = _chat_path(workspace_path, chat_id)
    tpath = _trace_path(workspace_path, chat_id)
    upath = _usage_path(workspace_path, chat_id)
    deleted = False
    if os.path.isfile(fpath):
        try:
            os.remove(fpath)
            deleted = True
        except Exception:
            pass
    if os.path.isfile(tpath):
        try:
            os.remove(tpath)
            deleted = True
        except Exception:
            pass
    if os.path.isfile(upath):
        try:
            os.remove(upath)
            deleted = True
        except Exception:
            pass
    return deleted


def update_chat_title(workspace_path: str, chat_id: str, title: str) -> bool:
    """Update just the title of a chat."""
    chat = get_chat(workspace_path, chat_id, include_traces=False, include_usage=False)
    if chat is None:
        return False
    chat["title"] = title
    return save_chat(workspace_path, chat_id, chat)


def cleanup_orphaned_chat_dirs() -> int:
    """Remove chat directories for workspaces no longer in the registry.

    Called on server startup to prevent zombie directories from
    accumulating after workspace deletions that didn't clean up.
    Returns the number of directories removed.
    """
    import shutil
    reg = _load_registry()
    active_ids = {ws.get("id", "") for ws in reg.get("workspaces", [])}
    central_chats_root = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "chats")
    removed = 0
    if os.path.isdir(central_chats_root):
        for dir_name in os.listdir(central_chats_root):
            if not dir_name.startswith("ws-"):
                continue
            if dir_name in active_ids:
                continue  # Still registered — keep
            dir_path = os.path.join(central_chats_root, dir_name)
            if os.path.isdir(dir_path):
                try:
                    shutil.rmtree(dir_path)
                    removed += 1
                    print(f"[CLEANUP] Removed orphaned chat directory: {dir_name}")
                except Exception as e:
                    print(f"[CLEANUP] Failed to remove {dir_name}: {e}")
    return removed


# ═══════════════════════════════════════════════════════════════════════════════
# Initialization — ensure at least the default workspace exists
# ═══════════════════════════════════════════════════════════════════════════════

def probe_environment(workspace_path: str) -> dict:
    """
    Checks for the presence of CLI tools and scans common ports.
    Saves this environment profile to workspace_memory.json.
    """
    import subprocess
    import socket
    from memory.memory_io import load_memory, save_memory
    
    # 1. Check CLI tools
    tools = ["composer", "npm", "php", "python", "go", "git"]
    tool_status = {}
    for tool in tools:
        try:
            # Under Windows, shell=True helps resolve paths for scripts/executables
            res = subprocess.run(
                f"{tool} --version",
                shell=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=2
            )
            if res.returncode == 0 or res.stdout or res.stderr:
                out = (res.stdout or res.stderr).strip().split('\n')[0]
                tool_status[tool] = {"available": True, "version": out[:100]}
            else:
                tool_status[tool] = {"available": False}
        except Exception:
            if tool == "python":
                try:
                    res = subprocess.run(
                        "python -V",
                        shell=True,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                        text=True,
                        timeout=2
                    )
                    if res.returncode == 0 or res.stdout or res.stderr:
                        out = (res.stdout or res.stderr).strip()
                        tool_status[tool] = {"available": True, "version": out}
                        continue
                except Exception:
                    pass
            tool_status[tool] = {"available": False}

    # 2. Port scan (commonly used by dev servers: 8000, 3000, 5173, 8080)
    ports = [8000, 3000, 5173, 8080]
    port_status = {}
    for port in ports:
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(0.5)
        res = s.connect_ex(('127.0.0.1', port))
        s.close()
        port_status[str(port)] = "IN_USE" if res == 0 else "FREE"

    profile = {
        "cli_tools": tool_status,
        "ports": port_status,
        "timestamp": datetime.now().isoformat()
    }

    # 3. Save to workspace_memory.json
    try:
        mem = load_memory(workspace_path)
        mem["environment_profile"] = profile
        save_memory(mem, workspace_path)
    except Exception as e:
        print(f"Failed to save environment profile to memory: {e}")

    return profile


def init_default_workspace() -> dict | None:
    """Ensure the default LangChain workspace is registered, but only if no workspaces exist."""
    default_path = r"d:\MyProject\LangChain"
    workspaces = list_workspaces()
    for ws in workspaces:
        if os.path.normpath(ws.get("path", "")) == os.path.normpath(default_path):
            try:
                probe_environment(default_path)
            except Exception:
                pass
            return ws

    # If other workspaces are registered, do not recreate the default workspace.
    if workspaces:
        return None

    ws = add_workspace(default_path, "LangChain")
    try:
        probe_environment(default_path)
    except Exception:
        pass
    return ws


def _resolve_workspace_id(workspace_path: str) -> str | None:
    """Find workspace ID from its path."""
    reg = _load_registry()
    norm = os.path.normpath(workspace_path).lower() if workspace_path else ""
    for ws in reg.get("workspaces", []):
        if os.path.normpath(ws.get("path", "")).lower() == norm:
            return ws.get("id")
    return None


def get_workspace_rules_and_profile(workspace_path: str) -> str:
    """
    Loads global user_profile.json and central workspace rules.json
    and formats them as a clean prompt injection block.
    """
    import json

    # Paths
    server_dir = r"d:\MyProject\LangChain"
    global_path = os.path.join(server_dir, ".deep_agents", "user_profile.json")

    # Rules stored centrally by workspace ID — never inside project folders
    rules_path = None
    ws_id = _resolve_workspace_id(workspace_path) if workspace_path else None
    if ws_id:
        rules_path = os.path.join(server_dir, ".deep_agents", "rules", f"{ws_id}.json")

    # Load global profile
    global_data = {}
    if os.path.isfile(global_path):
        try:
            with open(global_path, "r", encoding="utf-8") as f:
                global_data = json.load(f)
        except Exception:
            pass

    # Load central workspace rules
    local_data = {}
    if rules_path and os.path.isfile(rules_path):
        try:
            with open(rules_path, "r", encoding="utf-8") as f:
                local_data = json.load(f)
        except Exception:
            pass

    # Build prompt string
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

    # Load and append environment profile
    env_profile_data = {}
    try:
        from memory.memory_io import load_memory
        mem = load_memory(workspace_path)
        env_profile_data = mem.get("environment_profile", {})
    except Exception:
        pass

    if env_profile_data:
        sections.append("## System Environment Profile")
        cli_tools = env_profile_data.get("cli_tools", {})
        if cli_tools:
            available_tools = []
            for tool, info in cli_tools.items():
                if info.get("available"):
                    raw_ver = info.get("version") or "unknown version"
                    import re
                    match = re.search(r'\b\d+(?:\.\d+)+\b', raw_ver)
                    ver = match.group(0) if match else raw_ver.split('\n')[0][:15].strip()
                    available_tools.append(f"{tool} ({ver})")
            if available_tools:
                sections.append(f"- Available CLI Tools: {', '.join(available_tools)}")
        
        ports = env_profile_data.get("ports", {})
        if ports:
            conflict_ports = []
            for port, status in ports.items():
                if status == "IN_USE":
                    conflict_ports.append(f"Port {port}: IN_USE (Conflict warning! Another service is running on this port. Do NOT run server on this port.)")
            if conflict_ports:
                sections.append("### Dev Server Ports Conflict Warnings")
                for cp in conflict_ports:
                    sections.append(f"- {cp}")

    if sections:
        return "\n".join([
            "=========================================",
            "CRITICAL: USER PREFERENCES & RULES",
            "=========================================",
            ""
        ] + sections + [
            "",
            "========================================="
        ])
    return ""


def get_client_name(workspace_path: str = "") -> str:
    """Get the client name from global user_profile.json, falling back to a default."""
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


