"""
Project Router — direct-to-Dev routing for existing projects.

Two decisions, two owners:

- **New project creation** is decided ONLY by the orchestrator LLM: it calls
  the ``create_project`` tool when the user asks for a new project. Nothing in
  this module ever creates a folder from keywords — there is no hardcoded
  "coding word → create folder" trigger.
- **Existing project work** is routed deterministically and FREE: when the
  message comes from a registered project's chat, or explicitly mentions a
  registered project name, the pre-router sends it straight into that
  project's durable Dev child session (``subagent_manager.send_message``) —
  zero orchestrator tokens, context preserved across turns.

The index lives in ``.deep_agents/project_agents.json`` and is rebuilt from
durable child sessions on demand, so projects survive server restarts.
"""

from __future__ import annotations

import os
import re
import threading
from datetime import datetime

SERVER_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECTS_ROOT = r"D:\MyProject"
DFD_CLIENTS_ROOT = r"D:\MyProject\DFDClientProjects"
INDEX_PATH = os.path.join(SERVER_DIR, ".deep_agents", "project_agents.json")

#: Creation phrasing that MUST go to the orchestrator LLM (it decides whether
#: to actually create anything). Never create a project from these.
_CREATION_RE = re.compile(
    r"(\b(?:new|another|second|third)\s+project\b)"
    r"|(\b(?:create|make|start|build|scaffold)\s+(?:a\s+|an\s+|another\s+|the\s+|new\s+)?project\b)"
    r"|(\bproject\s+(?:called|named)\b)"
    r"|(\bproject\s*[:：])",
    re.IGNORECASE,
)

_LOCK = threading.RLock()


# ═══════════════════════════════════════════════════════════════════════════════
# Project index
# ═══════════════════════════════════════════════════════════════════════════════


def _load_index() -> dict:
    try:
        with open(INDEX_PATH, "r", encoding="utf-8") as f:
            import json
            return json.load(f)
    except Exception:
        return {"projects": []}


def _save_index(data: dict) -> None:
    try:
        os.makedirs(os.path.dirname(INDEX_PATH), exist_ok=True)
        import json
        with open(INDEX_PATH, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2)
        try:
            from routing.cache import invalidate as _cache_invalidate
            _cache_invalidate()
        except Exception:
            pass
    except Exception as e:
        print(f"[PROJECT_ROUTER] Index save error (non-fatal): {e}")


def _discover_from_sessions() -> list[dict]:
    """Scan every registered workspace for a continuable 'Dev' child and upsert
    the index (children persist as JSONL sessions, so this survives restarts)."""
    discovered: list[dict] = []
    try:
        import workspace_manager as wm
        from agents.subagent_manager import list_agents
        for ws in wm.list_workspaces():
            path = ws.get("path", "")
            if not path or not os.path.isdir(path):
                continue
            try:
                agents = list_agents(path, chat_id=None)
            except Exception as e:
                print(f"[PROJECT_ROUTER] list_agents skipped for {path}: {e}")
                continue
            for a in agents:
                if (a.get("name") or "") != "Dev":
                    continue
                discovered.append({
                    "name": ws.get("name", os.path.basename(path)),
                    "workspace_id": ws.get("id", ""),
                    "workspace_path": path,
                    "chat_id": a.get("parent_chat_id", ""),
                    "child_id": a.get("child_id", ""),
                    "created": datetime.now().isoformat(),
                })
    except Exception as e:
        print(f"[PROJECT_ROUTER] Session discovery error (non-fatal): {e}")
    return discovered


def list_projects() -> list[dict]:
    """Index entries merged with on-disk session discovery (deduped by path). Cached 3s."""
    try:
        from routing.cache import get_cached
        def _load():
            idx = _load_index()
            by_path: dict[str, dict] = {}
            for p in idx.get("projects", []):
                if p.get("workspace_path"):
                    by_path[os.path.normpath(p["workspace_path"]).lower()] = p
            for p in _discover_from_sessions():
                if p.get("workspace_path"):
                    by_path[os.path.normpath(p["workspace_path"]).lower()] = p
            return sorted(by_path.values(), key=lambda p: p.get("name", "").lower())
        return get_cached(_load)
    except Exception:
        idx = _load_index()
        by_path: dict[str, dict] = {}
        for p in idx.get("projects", []):
            if p.get("workspace_path"):
                by_path[os.path.normpath(p["workspace_path"]).lower()] = p
        for p in _discover_from_sessions():
            if p.get("workspace_path"):
                by_path[os.path.normpath(p["workspace_path"]).lower()] = p
        return sorted(by_path.values(), key=lambda p: p.get("name", "").lower())


def _norm(path: str) -> str:
    return os.path.normpath(os.path.abspath(path or "")).lower()


def find_project_by_workspace(workspace_path: str) -> dict | None:
    if not workspace_path:
        return None
    target = _norm(workspace_path)
    for p in list_projects():
        if _norm(p.get("workspace_path", "")) == target:
            return p
    return None


def find_projects_in_text(text: str) -> dict | None:
    """Best-match a registered project name mentioned in free text.

    Exact/case-insensitive match wins; falls back to a ``project <name>``
    pattern. Longest name wins so short names can't shadow longer ones.
    """
    if not text:
        return None
    lowered = text.lower()
    projects = list_projects()
    if not projects:
        return None

    def _name_tokens(name: str) -> list[str]:
        return re.findall(r"[a-z0-9_.\-]+", name.lower())

    best = None
    best_len = 0
    for p in projects:
        name = (p.get("name") or "").strip()
        if not name:
            continue
        nl = name.lower()
        # quoted / explicit "project X" mention
        if re.search(rf"(^|[\s(（\[\"'`])(project\s*[\"']?)?{re.escape(nl)}([\"']?[\s)）\].,;:!?]|$)", text, re.IGNORECASE) and len(nl) > best_len:
            best, best_len = p, len(nl)
            continue
        # word-boundary contains (token match) — avoid substring false hits
        tokens = _name_tokens(name)
        if tokens and all(t in lowered for t in tokens) and len(nl) > best_len:
            best, best_len = p, len(nl)
    return best


def _creation_phrasing(text: str) -> bool:
    """True when the prompt looks like project-CREATION phrasing — in that
    case the orchestrator LLM decides (it may call create_project, or decide
    the user means something else). We never create from here."""
    return bool(_CREATION_RE.search(text or ""))


def route_request(prompt: str, workspace_path: str, chat_id: str = "") -> tuple:
    """Decide who answers this message.

    Returns ``("project_dev", project)`` for a direct send to an existing
    project's Dev child, or ``("orchestrator", None)`` for everything else.
    """
    if _creation_phrasing(prompt):
        return ("orchestrator", None)

    # 1) Message sent from inside a registered project chat → its Dev child.
    if workspace_path:
        project = find_project_by_workspace(workspace_path)
        if project and project.get("child_id"):
            return ("project_dev", project)

    # 2) Message explicitly names a registered project → that project's Dev.
    project = find_projects_in_text(prompt or "")
    if project and project.get("child_id"):
        return ("project_dev", project)

    return ("orchestrator", None)


# ═══════════════════════════════════════════════════════════════════════════════
# Project creation (orchestrator-tool only)
# ═══════════════════════════════════════════════════════════════════════════════


def _sanitize_name(name: str) -> str:
    clean = re.sub(r'[\\/:*?"<>|\r\n]+', "_", (name or "").strip())
    clean = re.sub(r"\s+", " ", clean).strip().rstrip(".")
    return clean[:80]


def _update_entry_child(workspace_path: str, child_id: str, first_reply: str) -> None:
    """Persist a background-spawned Dev child onto its index entry (best-effort)."""
    try:
        with _LOCK:
            idx = _load_index()
            key = os.path.normpath(workspace_path).lower()
            for p in idx.get("projects", []):
                if os.path.normpath(p.get("workspace_path", "")).lower() == key:
                    p["child_id"] = child_id
                    break
            _save_index(idx)
            try:
                from routing.cache import invalidate as _cache_invalidate
                _cache_invalidate()
            except Exception:
                pass
    except Exception as e:
        print(f"[PROJECT_ROUTER] Background child persist error (non-fatal): {e}")
    if first_reply:
        try:
            entry = next((p for p in _load_index().get("projects", [])
                          if os.path.normpath(p.get("workspace_path", "")).lower()
                          == os.path.normpath(workspace_path).lower()), None)
            if entry:
                import workspace_manager as wm
                wm.add_message(entry.get("workspace_path", ""), entry.get("chat_id", ""),
                               "agent", first_reply, {"routed": True, "first_turn": True})
        except Exception as e:
            print(f"[PROJECT_ROUTER] First-reply mirror error (non-fatal): {e}")


def _spawn_first_turn_background(workspace_path: str, chat_id: str, concept: str) -> None:
    """Run the durable Dev's FIRST TURN in a background thread (wait=false)."""
    def _worker():
        try:
            from agents.subagent_manager import start_continuable
            child = start_continuable(
                chat_id, workspace_path, "Dev",
                task=concept or "Get familiar with the empty project and report what you will build.",
                label=f"{os.path.basename(workspace_path)} · Dev",
            )
            _update_entry_child(workspace_path, child.get("child_id", ""), child.get("result", ""))
        except Exception as e:
            print(f"[PROJECT_ROUTER] Background first-turn error (non-fatal): {e}")

    import threading
    threading.Thread(target=_worker, daemon=True, name="project-first-turn").start()


# ── Stage 5: create_project scaffold (.gitignore + README) ──────────────────

_GITIGNORE_PYTHON = """# Byte-compiled / optimized / DLLs
__pycache__/
*.py[cod]
*$py.class
*.so

# Distribution / packaging
.Python
build/
dist/
*.egg-info/
*.egg
.eggs/
.installed.cfg
*.egg-info
pip-log.txt
pip-delete-this-directory.txt

# Unit test / coverage
htmlcov/
.tox/
.nox/
.coverage
.coverage.*
.cache
.pytest_cache/
.mypy_cache/
.ruff_cache/
.pyright_cache/

# Environments
.env
.env.local
.env.*.local
.venv/
venv/
env/
ENV/

# IDE / editor
.idea/
.vscode/
*.swp
*.swo
.DS_Store
Thumbs.db

# Logs
*.log
logs/

# Jupyter
.ipynb_checkpoints
"""

_GITIGNORE_NODE = """# Dependencies
node_modules/
.pnp
.pnp.js
.yarn/install-state.gz

# Build outputs
.next/
out/
dist/
build/

# Environments
.env
.env.local
.env.*.local

# Logs
*.log
npm-debug.log*
yarn-debug.log*
yarn-error.log*
pnpm-debug.log*

# IDE / editor
.idea/
.vscode/
*.swp
*.swo
.DS_Store
Thumbs.db

# Test / coverage
coverage/
.nyc_output/

# Cache
.turbo/
.cache/
.parcel-cache/
"""

_GITIGNORE_GENERIC = """# Editors
.idea/
.vscode/
*.swp
*.swo
.DS_Store
Thumbs.db

# Logs
*.log

# Environments
.env
.env.local
.env.*.local
"""

_README_TEMPLATE = """# {name}

> {concept}

## Getting started

This project was scaffolded by the LangChain agent harness. Replace this
README with project-specific setup, usage, and architecture notes.
"""


def _detect_language(path: str) -> str:
    """Heuristic: scan one level of the new folder for source files and
    return `python`, `node`, or `generic`. Only looks at common extensions
    and is deliberately conservative."""
    try:
        for entry in os.listdir(path):
            full = os.path.join(path, entry)
            if not os.path.isfile(full):
                continue
            if entry.endswith((".py", ".pyi")):
                return "python"
            if entry.endswith((".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs")):
                return "node"
    except OSError:
        pass
    return "node"  # default per user direction


def _scaffold_gitignore_and_readme(path: str, name: str, concept: str) -> None:
    """Drop a language-appropriate `.gitignore` and a `README.md`. Skips
    both files if they already exist (so a user who pre-populated the
    folder isn't overwritten)."""
    gitignore_path = os.path.join(path, ".gitignore")
    if not os.path.isfile(gitignore_path):
        lang = _detect_language(path)
        if lang == "python":
            content = _GITIGNORE_PYTHON
        elif lang == "node":
            content = _GITIGNORE_NODE
        else:
            content = _GITIGNORE_GENERIC
        with open(gitignore_path, "w", encoding="utf-8") as f:
            f.write(content)

    readme_path = os.path.join(path, "README.md")
    if not os.path.isfile(readme_path):
        with open(readme_path, "w", encoding="utf-8") as f:
            f.write(_README_TEMPLATE.format(name=name, concept=concept or "(no concept provided)"))


def create_project(name: str, concept: str, chat_id: str = "", wait: bool = True, parent_root: str = "") -> dict:
    """Create a project folder + workspace + chat + durable Dev child.

    ONLY the orchestrator LLM (or the DFD agent, inside a client root) calls
    this (via the ``create_project`` tool). ``wait=False`` registers
    everything and runs the Dev's first turn in the background — use it when
    creating SEVERAL projects in one burst so they spin up in parallel.
    Returns an error dict on invalid input.

    ``parent_root`` — if provided, the project folder is created under it
    instead of the default ``D:\\MyProject`` root. The DFD agent passes the
    client folder ``D:\\MyProject\\DFDClientProjects\\<slug>\\`` so client
    projects ALWAYS live inside the fixed client root.
    """
    safe = _sanitize_name(name)
    if not safe:
        return {"error": "Project name is empty after sanitizing. Provide a valid name."}
    if safe.lower() in ("general", "_general"):
        return {"error": "'General' is reserved for the general chat workspace."}

    # Scope guard: if a parent_root is requested, it MUST be under the DFD
    # client root. Never create coding workspaces outside it for client work.
    if parent_root:
        client_root = os.path.normpath(DFD_CLIENTS_ROOT).lower()
        norm_parent = os.path.normpath(parent_root).lower()
        if not norm_parent.startswith(client_root + os.sep):
            return {"error": f"create_project parent_root must be inside {DFD_CLIENTS_ROOT}; got '{parent_root}'."}

    path = os.path.join(parent_root or PROJECTS_ROOT, safe)
    created_folder = False
    try:
        if os.path.exists(path) and not os.path.isdir(path):
            return {"error": f"Cannot create project: '{path}' exists and is not a directory."}
        if not os.path.isdir(path):
            os.makedirs(path, exist_ok=True)
            created_folder = True
    except Exception as e:
        return {"error": f"Failed to create project folder '{path}': {e}"}

    with _LOCK:
        try:
            # Idempotent: the project already exists with a live Dev child →
            # reuse it. Prevents duplicate Dev agents for the same folder.
            existing = next(
                (p for p in _load_index().get("projects", [])
                 if _norm(p.get("workspace_path", "")) == _norm(path) and p.get("child_id")),
                None,
            )
            if existing:
                existing["already_existed"] = True
                return existing

            import workspace_manager as wm
            ws = wm.add_workspace(path, safe)
            chat = wm.create_chat(path, title=f"{safe} — Dev Session")
        except Exception as e:
            return {"error": f"Failed to set up project '{safe}': {e}"}

    entry = {
        "name": safe,
        "workspace_id": ws.get("id", ""),
        "workspace_path": path,
        "chat_id": chat.get("id", ""),
        "child_id": "",
        "created": datetime.now().isoformat(),
    }
    try:
        idx = _load_index()
        key = os.path.normpath(path).lower()
        idx["projects"] = [p for p in idx.get("projects", []) if os.path.normpath(p.get("workspace_path", "")).lower() != key]
        idx["projects"].append(entry)
        _save_index(idx)
    except Exception as e:
        print(f"[PROJECT_ROUTER] Index persist error (non-fatal): {e}")

    if created_folder:
        try:
            import subprocess
            subprocess.run(["git", "init", "-q", path], timeout=30, capture_output=True, check=False)
        except Exception as e:
            print(f"[PROJECT_ROUTER] git init skipped for {path}: {e}")

        # Stage 5: drop a language-appropriate .gitignore + a README.md.
        # Detected from the first source-file extension found in the new
        # folder (default: Node/Next). Fails silently — the user can
        # always replace them.
        try:
            _scaffold_gitignore_and_readme(path, safe, concept)
        except Exception as e:
            print(f"[PROJECT_ROUTER] scaffold skipped for {path}: {e}")

    if wait:
        try:
            from agents.subagent_manager import start_continuable
            child = start_continuable(
                chat["id"], path, "Dev",
                task=concept or "Get familiar with the empty project and report what you will build.",
                label=f"{safe} · Dev",
            )
            entry["child_id"] = child.get("child_id", "")
            # Persist the resolved child id back onto the stored entry.
            try:
                with _LOCK:
                    idx = _load_index()
                    for p in idx.get("projects", []):
                        if os.path.normpath(p.get("workspace_path", "")).lower() == key:
                            p["child_id"] = entry["child_id"]
                            break
                    _save_index(idx)
            except Exception:
                pass
            entry["first_reply"] = child.get("result", "")
        except Exception as e:
            return {"error": f"Failed to set up project '{safe}': {e}"}
    else:
        _spawn_first_turn_background(path, chat["id"], concept)
        entry["background"] = True

    # Register the project onto its client index entry when created inside a
    # DFD client root (best-effort; non-fatal).
    if parent_root:
        try:
            from agents.client_manager import add_project_to_client
            slug = os.path.basename(os.path.normpath(parent_root))
            add_project_to_client(slug, safe, path)
        except Exception:
            pass

    return entry


def find_project_dev(name: str) -> dict | None:
    """Resolve a project by name/workspace and return its Dev child info."""
    p = find_projects_in_text(name or "") or find_project_by_workspace(name or "")
    if not p:
        return None
    return {
        "name": p.get("name", ""),
        "workspace_id": p.get("workspace_id", ""),
        "workspace_path": p.get("workspace_path", ""),
        "chat_id": p.get("chat_id", ""),
        "child_id": p.get("child_id", ""),
    }


# ═══════════════════════════════════════════════════════════════════════════════
# Direct sends & streaming
# ═══════════════════════════════════════════════════════════════════════════════


def summarize(text: str, limit: int = 600) -> str:
    text = (text or "").strip()
    if len(text) <= limit:
        return text
    return text[:limit].rstrip() + "…"


def send_to_project(project: dict, message: str, source_workspace: str = "", source_chat: str = "") -> dict:
    """Deliver a message straight into a project's Dev child session.

    The FULL reply is mirrored into the project's own chat; the returned
    ``summary`` is what the source chat should show.
    """
    child_id = project.get("child_id", "")
    ws = project.get("workspace_path", "")
    if not child_id or not ws:
        return {"error": "Project has no active Dev agent assigned yet."}
    try:
        from agents.subagent_manager import send_message as _sm
        r = _sm(child_id, message, ws, timeout=float(os.environ.get("PROJECT_DEV_TIMEOUT", "1800")))
    except Exception as e:
        return {"error": f"Failed to reach project Dev agent: {e}"}
    if r.get("status") == "error":
        return {"error": r.get("result", "Unknown child error")}
    if r.get("queued"):
        return {"error": "Project Dev agent is busy; try again in a moment."}
    reply = r.get("result", "")
    try:
        import workspace_manager as wm
        wm.add_message(ws, project.get("chat_id", ""), "agent", reply, {"routed": True, "from_chat": source_chat})
    except Exception as e:
        print(f"[PROJECT_ROUTER] Mirror-to-project-chat error (non-fatal): {e}")
    return {"reply": reply, "summary": summarize(reply), "project": project}


def _project_ref(project: dict, reply: str) -> dict:
    return {
        "name": project.get("name", ""),
        "workspace_id": project.get("workspace_id", ""),
        "chat_id": project.get("chat_id", ""),
        "path": project.get("workspace_path", ""),
        "full_reply": reply,
    }


def stream_direct(project: dict, message: str, workspace_path: str, chat_id: str = ""):
    """Generator yielding state snapshots for a direct-to-Dev run.

    Mirrors the shape of ``sync_manager.run_and_sync_graph`` so both the API
    stream and the graph wrapper can consume it.
    """
    from sync.state_sync import safe_get_state, safe_update_state, shared_state

    is_own_chat = bool(chat_id) and chat_id == project.get("chat_id", "")

    safe_update_state({
        "active_node": "agent",
        "next_agent": "",
        "client_request": message,
        "project_path": project.get("workspace_path", ""),
        "thoughts": {"agent": f"Routing straight to Dev agent of '{project.get('name', '')}'..."},
        "live_terminal_log": f"[>] User: {message}\n",
        "outputs": {
            **shared_state.get("outputs", {}),
            "agent_report": "",
        },
    })
    yield safe_get_state()

    safe_append_live_log(f"\n[ROUTED] Direct to Dev agent of project '{project.get('name', '')}' (no orchestrator).")
    result = send_to_project(project, message, workspace_path, chat_id)
    if result.get("error"):
        safe_append_live_log(f"\n[ASSISTANT] ERROR: {result['error']}")
        safe_update_state({"outputs": {**shared_state.get("outputs", {}), "agent_report": f"ERROR: {result['error']}"}})
        yield safe_get_state()
        return

    reply = result.get("reply", "")
    if is_own_chat:
        report = reply
        pr = None
    else:
        report = result.get("summary", "")
        pr = _project_ref(project, reply)

    safe_append_live_log(f"\n[ASSISTANT] {report}")
    safe_update_state({
        "active_node": "agent",
        "thoughts": {"agent": "Work complete."},
        "project_ref": pr,
        "outputs": {**shared_state.get("outputs", {}), "agent_report": report},
    })
    yield safe_get_state()


def safe_append_live_log(msg: str) -> None:
    try:
        from sync.state_sync import safe_append_live_log as _sa
        _sa(msg)
    except Exception as e:
        print(f"[PROJECT_ROUTER] live-log append skipped: {e}")
