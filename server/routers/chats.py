"""
Chats + subagents + files router â€” Phase 7 moved from server/api_server.py
(chats:1468-1590, subagents:1500-1526, files:1261-1315).
"""
import os
import uuid
from fastapi import APIRouter, File, Request, UploadFile; from fastapi.responses import JSONResponse

from server.store.workspace_registry import get_workspace
from server.store.chat_store import (
    list_chats, get_chat, create_chat, delete_chat, add_message,
    update_chat_title, save_chat,
)

router = APIRouter(tags=["chats"])

_ROOT = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

_FILE_SKIP_DIRS = {
    ".git", ".hg", ".svn", "node_modules", "venv", "venv312", ".venv",
    "__pycache__", ".pytest_cache", ".ruff_cache", ".agent-memory",
    ".deep_agents", "dist", "build", ".next", "target", "release",
}
_FILE_MAX_SIZE = 5 * 1024 * 1024  # skip files larger than 5 MB in listings


@router.get("/api/workspaces/{workspace_id}/chats")
async def get_chats(workspace_id: str):
    ws = get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    chats = list_chats(ws["path"])
    return JSONResponse(content={"chats": chats})


@router.post("/api/workspaces/{workspace_id}/chats")
async def create_chat_endpoint(workspace_id: str, req: Request):
    ws = get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    body = await req.json()
    title = body.get("title", "")
    model = body.get("model", "Automatic Fallback")
    chat = create_chat(ws["path"], title, model)
    return JSONResponse(content=chat)


@router.get("/api/workspaces/{workspace_id}/chats/{chat_id}")
async def get_chat_endpoint(workspace_id: str, chat_id: str):
    ws = get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    chat = get_chat(ws["path"], chat_id)
    if not chat:
        return JSONResponse(content={"error": "Chat not found"}, status_code=404)
    return JSONResponse(content=chat)


@router.get("/api/workspaces/{workspace_id}/chats/{chat_id}/subagents")
async def list_chat_subagents(workspace_id: str, chat_id: str):
    """Children spawned by this chat (cont- continuable + sub- task spawns)."""
    ws = get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    try:
        from agents.subagent_manager import list_agents
        return JSONResponse(content={"subagents": list_agents(ws["path"], chat_id)})
    except Exception as e:
        return JSONResponse(content={"subagents": [], "error": str(e)})


@router.get("/api/workspaces/{workspace_id}/subagents/{child_id}")
async def get_subagent_session(workspace_id: str, child_id: str):
    """Read-only transcript of one spawned subagent session (drill-in view)."""
    ws = get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    try:
        from agents.subagent_manager import read_child_session
        data = read_child_session(ws["path"], child_id)
    except Exception:
        data = None
    if not data:
        return JSONResponse(content={"error": "Subagent session not found"}, status_code=404)
    return JSONResponse(content=data)


@router.delete("/api/workspaces/{workspace_id}/chats/{chat_id}")
async def delete_chat_endpoint(workspace_id: str, chat_id: str):
    ws = get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    ok = delete_chat(ws["path"], chat_id)
    return JSONResponse(content={"status": "ok" if ok else "not found"})


@router.post("/api/workspaces/{workspace_id}/chats/{chat_id}/messages")
async def add_message_endpoint(workspace_id: str, chat_id: str, req: Request):
    ws = get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    body = await req.json()
    role = body.get("role", "user")
    content = body.get("content", "")
    meta = body.get("metadata")
    msg = add_message(ws["path"], chat_id, role, content, meta)
    if not msg:
        return JSONResponse(content={"error": "Chat not found"}, status_code=404)
    return JSONResponse(content=msg)


@router.post("/api/workspaces/{workspace_id}/chats/{chat_id}/attachments")
async def upload_chat_attachments(workspace_id: str, chat_id: str, files: list[UploadFile] = File(...)):
    """Store chat attachments outside the workspace and return safe agent-readable paths."""
    ws = get_workspace(workspace_id)
    if not ws or not get_chat(ws["path"], chat_id):
        return JSONResponse(content={"error": "Chat not found"}, status_code=404)
    if len(files) > 8:
        return JSONResponse(content={"error": "A maximum of 8 files can be uploaded"}, status_code=400)

    upload_dir = os.path.join(_ROOT, ".deep_agents", "uploads", workspace_id, chat_id)
    os.makedirs(upload_dir, exist_ok=True)
    attachments = []
    for item in files:
        filename = os.path.basename(item.filename or "attachment")
        contents = await item.read()
        if len(contents) > 20 * 1024 * 1024:
            return JSONResponse(content={"error": f"{filename} is larger than 20 MB"}, status_code=400)
        stored_name = f"{uuid.uuid4().hex[:8]}-{filename}"
        target = os.path.join(upload_dir, stored_name)
        with open(target, "wb") as handle:
            handle.write(contents)
        mime = item.content_type or "application/octet-stream"
        attachments.append({
            "id": stored_name, "name": filename, "type": mime, "size": len(contents),
            "path": target, "isImage": mime.startswith("image/"),
        })
    return JSONResponse(content={"attachments": attachments})


@router.put("/api/workspaces/{workspace_id}/chats/{chat_id}/title")
async def update_chat_title(workspace_id: str, chat_id: str, req: Request):
    ws = get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    body = await req.json()
    title = body.get("title", "")
    ok = update_chat_title(ws["path"], chat_id, title)
    return JSONResponse(content={"status": "ok" if ok else "not found"})


@router.get("/api/workspaces/{workspace_id}/files")
async def list_workspace_files(workspace_id: str, max_depth: int = 8):
    """Flat listing of a workspace folder tree (dirs then files, depth-capped)."""
    ws = get_workspace(workspace_id)
    if not ws or not ws.get("path") or not os.path.isdir(ws["path"]):
        return JSONResponse(content={"files": [], "error": "Workspace folder not found"}, status_code=404)
    root = ws["path"]
    out = []

    def walk(d: str, depth: int, prefix: str) -> None:
        if depth > max_depth:
            return
        try:
            entries = sorted(os.listdir(d), key=str.lower)
        except Exception:
            return
        for name in entries:
            if name in _FILE_SKIP_DIRS:
                continue
            full = os.path.join(d, name)
            rel = f"{prefix}/{name}" if prefix else name
            try:
                if os.path.isdir(full):
                    out.append({"name": name, "path": rel.replace("\\", "/"), "type": "dir"})
                    walk(full, depth + 1, rel)
                else:
                    size = os.path.getsize(full)
                    if size > _FILE_MAX_SIZE:
                        continue
                    out.append({"name": name, "path": rel.replace("\\", "/"), "type": "file", "size": size})
            except Exception:
                continue

    walk(root, 0, "")
    # Authored project memory lives in a skipped metadata dir; expose the one
    # file the design workspace previews as the brand contract.
    _pm = os.path.join(root, ".agent-memory", "project.md")
    if os.path.isfile(_pm):
        try:
            out.append({"name": "project.md", "path": ".agent-memory/project.md",
                        "type": "file", "size": os.path.getsize(_pm)})
        except Exception:
            pass
    out.sort(key=lambda f: (f["type"] != "dir", f["path"].lower()))
    return JSONResponse(content={"files": out})


@router.get("/api/workspaces/{workspace_id}/file")
async def get_workspace_file(workspace_id: str, path: str = ""):
    """Read one workspace file (path-traversal guarded, 200 KB read cap)."""
    ws = get_workspace(workspace_id)
    if not ws or not ws.get("path"):
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    root = os.path.realpath(ws["path"])
    candidate = os.path.realpath(os.path.join(root, path))
    try:
        if os.path.commonpath([root, candidate]) != root:
            return JSONResponse(content={"error": "Path outside workspace"}, status_code=400)
    except ValueError:
        return JSONResponse(content={"error": "Invalid path"}, status_code=400)
    if not os.path.isfile(candidate):
        return JSONResponse(content={"error": "File not found"}, status_code=404)
    size = os.path.getsize(candidate)
    if size > 200 * 1024:
        return JSONResponse(content={"error": "File too large to preview"}, status_code=413)
    try:
        with open(candidate, "r", encoding="utf-8", errors="replace") as f:
            content = f.read()
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)
    return JSONResponse(content={"path": path, "content": content})



