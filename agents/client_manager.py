"""
client_manager.py --- DFD client scaffolding under the fixed root.

Creates the per-client shell under ``D:\\MyProject\\DFDClientProjects\\<slug>\\``:

- ``client.json``  (name, vertical, features, whatsapp, domain, status)
- ``progress.md``  (human-readable status + decision log)
- ``mem/``         (client memory dir — shared by all this client's chats)

Gate: ``create_client`` MUST NOT be invoked without an explicit owner
approval. The approval flow is owned by the DFD agent (Telegram by default;
the DFD chat is the fallback). This module only materializes the approved
scaffold and records the decision in ``.deep_agents/dfd_clients.json``.
"""

from __future__ import annotations

import json
import os
import re
import threading
from datetime import datetime

SERVER_DIR = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DFD_CLIENTS_ROOT = r"D:\MyProject\DFDClientProjects"
INDEX_PATH = os.path.join(SERVER_DIR, ".deep_agents", "dfd_clients.json")

_LOCK = threading.RLock()

VALID_VERTICALS = {
    "fnb", "salon", "laundry", "klinik", "gym", "bimbel",
    "bengkel", "beauty", "travel", "property", "katering", "photo",
}


def _slugify(name: str) -> str:
    clean = re.sub(r"[^a-z0-9]+", "-", (name or "").strip().lower())
    return clean.strip("-") or "client"


def _load_index() -> dict:
    try:
        with open(INDEX_PATH, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return {"clients": []}


def _save_index(data: dict) -> None:
    try:
        os.makedirs(os.path.dirname(INDEX_PATH), exist_ok=True)
        with open(INDEX_PATH, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2)
    except Exception:
        pass


def list_clients() -> list[dict]:
    idx = _load_index()
    return sorted(idx.get("clients", []), key=lambda c: c.get("createdAt", ""), reverse=True)


def find_client(slug: str) -> dict | None:
    slug = _slugify(slug)
    for c in list_clients():
        if c.get("slug") == slug:
            return c
    return None


def create_client(
    business_name: str,
    vertical: str,
    features: list | None = None,
    whatsapp: str = "",
    domain: str = "",
    approved_by: str = "owner",
    order_id: str = "",
) -> dict:
    """Create the client scaffold under DFDClIENT_PROJECTS_ROOT.

    IMPORTANT: this function materializes an ALREADY-APPROVED scaffold. The
    DFD agent must obtain explicit owner approval before calling it. The
    ``approved_by`` field records who approved; a missing/empty approval
    marker is rejected.
    """
    approved_by = (approved_by or "").strip()
    # Gate: only a real owner-approval marker (e.g. "telegram:<request_id>"
    # or an explicit named approver) may create the scaffold. Empty/"owner"
    # means no approval was obtained — refuse.
    if not approved_by or approved_by.lower() in ("owner", "self", "auto", "yes", "approve", "approved"):
        return {"error": "create_client requires explicit owner approval (approved_by must be a real approval marker, e.g. 'telegram:<request_id>'). The DFD agent must obtain approval via the approval channel first."}

    name = (business_name or "").strip()
    if not name:
        return {"error": "business_name is required."}
    vertical = (vertical or "").strip().lower()
    if vertical not in VALID_VERTICALS:
        return {"error": f"vertical must be one of: {', '.join(sorted(VALID_VERTICALS))}."}

    slug = _slugify(name)
    with _LOCK:
        existing = find_client(slug)
        if existing:
            existing["already_existed"] = True
            return existing

        client_dir = os.path.join(DFD_CLIENTS_ROOT, slug)
        if os.path.exists(client_dir) and not os.path.isdir(client_dir):
            return {"error": f"Cannot create client: '{client_dir}' exists and is not a directory."}
        os.makedirs(os.path.join(client_dir, "mem"), exist_ok=True)

        now = datetime.now().isoformat()
        client = {
            "slug": slug,
            "name": name,
            "vertical": vertical,
            "features": sorted(set(features or [])),
            "whatsapp": (whatsapp or "").strip(),
            "domain": (domain or "").strip(),
            "status": "active",
            "orderId": order_id or "",
            "createdAt": now,
            "updatedAt": now,
            "projects": [],
        }

        # client.json
        with open(os.path.join(client_dir, "client.json"), "w", encoding="utf-8") as f:
            json.dump(client, f, indent=2)

        # progress.md
        progress_path = os.path.join(client_dir, "progress.md")
        if not os.path.isfile(progress_path):
            with open(progress_path, "w", encoding="utf-8") as f:
                f.write(f"# {name} — Progress\n\n- {now}: Client scaffold created (approved by {approved_by}).\n")

        # mem/README
        mem_readme = os.path.join(client_dir, "mem", "README.md")
        if not os.path.isfile(mem_readme):
            with open(mem_readme, "w", encoding="utf-8") as f:
                f.write("# Client Memory\n\nShared memory for all chats of this client.\n")

        idx = _load_index()
        idx["clients"] = [c for c in idx.get("clients", []) if c.get("slug") != slug]
        idx["clients"].append(client)
        _save_index(idx)

        return {**client, "path": client_dir}


def add_project_to_client(slug: str, project_name: str, project_path: str) -> dict:
    """Register a created project onto the client index entry (best-effort)."""
    slug = _slugify(slug)
    with _LOCK:
        idx = _load_index()
        for c in idx.get("clients", []):
            if c.get("slug") != slug:
                continue
            projects = c.setdefault("projects", [])
            if project_path not in projects:
                projects.append({"name": project_name, "path": project_path})
            c["updatedAt"] = datetime.now().isoformat()
            _save_index(idx)
            return {"ok": True, "slug": slug}
        return {"error": f"Client '{slug}' not found."}
